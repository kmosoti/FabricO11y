//! O8 deterministic private-worker controls; no transport-performance claim.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root = PathBuf::from(std::env::var("FABRIC_SCRATCH_ROOT").expect("contained scratch"));
        let path = root.join(format!(
            "o8-control-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn spindle(&self, cap: u64) -> Spindle {
        std::fs::write(self.0.join("input.log"), "first\n").unwrap();
        Spindle::open(Config {
            spool: self.0.join("spool"),
            logs: vec![self.0.join("input.log")],
            interval_s: 15,
            spool_bytes: cap,
            server: None,
            traces_listen: None,
            max_output_bytes_per_s: None,
        })
        .unwrap()
    }
    fn append(&self, text: &str) {
        use std::io::Write;
        OpenOptions::new()
            .append(true)
            .open(self.0.join("input.log"))
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn until() -> Instant {
    Instant::now() + Duration::from_secs(3)
}

#[test]
fn overlap_durable_successor_precedes_ack_without_second_send() {
    let s = Scratch::new();
    let mut spindle = s.spindle(1024 * 1024);
    spindle.collect_logs().unwrap().unwrap();
    let original = spindle.journal.next_unacked().unwrap().unwrap().1;
    s.append("second\n");
    let spool = s.0.join("spool");
    let spool_send = spool.clone();
    let stop = AtomicBool::new(false);
    let report = spindle
        .overlap_attempt(
            until(),
            false,
            &stop,
            |attempt| {
                assert_eq!(attempt.sequence, 1);
                assert_eq!(
                    Spool::inspect(&spool, 1024 * 1024, |_| Ok(()))
                        .unwrap()
                        .acked_through,
                    0
                );
            },
            move |bytes| {
                assert_eq!(bytes, original);
                let end = until();
                loop {
                    if let Ok(view) = Spool::inspect(&spool_send, 1024 * 1024, |_| Ok(()))
                        && view.next_sequence == 3
                    {
                        assert_eq!(view.acked_through, 0);
                        break;
                    }
                    assert!(
                        Instant::now() < end,
                        "successor did not become durable during ACK wait"
                    );
                    std::thread::yield_now();
                }
                Delivery::Ack(1)
            },
        )
        .unwrap();
    assert_eq!(report.delivery.sent, 1);
    assert_eq!(report.delivery.acked_through, 1);
    assert_eq!(report.prepared.unwrap().batch_sequence, 2);
    assert_eq!(spindle.journal.next_unacked().unwrap().unwrap().0, 2);
}

#[test]
fn overlap_retry_preserves_bytes_and_never_prepares_a_third_batch() {
    let s = Scratch::new();
    let mut spindle = s.spindle(1024 * 1024);
    spindle.collect_logs().unwrap();
    s.append("second\n");
    let original = spindle.journal.next_unacked().unwrap().unwrap().1;
    let stop = AtomicBool::new(false);
    let failed = spindle
        .overlap_attempt(
            until(),
            false,
            &stop,
            |_| {},
            |bytes| {
                assert_eq!(bytes, original);
                Delivery::Retry("lost ACK".into())
            },
        )
        .unwrap();
    assert_eq!(failed.delivery.acked_through, 0);
    assert!(failed.prepared.is_some());
    s.append("third must wait\n");
    let retry = spindle
        .overlap_attempt(
            until(),
            false,
            &stop,
            |_| {},
            |bytes| {
                assert_eq!(bytes, original);
                Delivery::Ack(1)
            },
        )
        .unwrap();
    assert!(retry.prepared.is_none());
    assert_eq!(spindle.journal.next_sequence(), 3);
    assert_eq!(spindle.journal.next_unacked().unwrap().unwrap().0, 2);
}

#[test]
fn overlap_full_spool_retains_ack_and_unadvanced_log_cursor() {
    let s = Scratch::new();
    let mut spindle = s.spindle(8192);
    spindle.collect_logs().unwrap();
    let cursors = spindle.cursors.clone();
    // Origin: catalog-coupled-o8-controls-01. One 6000-byte line was
    // skipped by the 4096-byte source limit, leaving only a small gap that
    // fit the Spool. Use valid lines whose combined bodies exceed its
    // 4096-byte journal allowance, and establish that precondition.
    s.append(&("x".repeat(2000) + "\n").repeat(3));
    let input = s.0.join("input.log");
    let read = log_source::read_lines_costed(
        &input,
        spindle.cursors.get(input.to_str().unwrap()),
        LOG_BODY_BUDGET,
        input.to_str().unwrap().len() + LOG_LINE_OVERHEAD,
    )
    .unwrap();
    assert!(read.gaps.is_empty());
    assert_eq!(read.lines.len(), 3);
    assert_eq!(
        read.lines.iter().map(|line| line.body.len()).sum::<usize>(),
        6000
    );
    assert!(6000 > spindle.config.journal_cap());
    let report = spindle
        .overlap_attempt(
            until(),
            false,
            &AtomicBool::new(false),
            |_| {},
            |_| Delivery::Ack(1),
        )
        .unwrap();
    assert!(report.collection_error.is_some());
    assert!(report.prepared.is_none());
    assert_eq!(report.delivery.acked_through, 1);
    assert_eq!(spindle.cursors, cursors);
    assert_eq!(spindle.journal.next_sequence(), 2);
    assert!(read_unknown(&s.0.join("spool")).is_some());
}

#[test]
fn overlap_stop_and_invalid_future_ack_preserve_custody() {
    let s = Scratch::new();
    let mut spindle = s.spindle(1024 * 1024);
    spindle.collect_logs().unwrap();
    s.append("second\n");
    let stop = AtomicBool::new(true);
    let stopped = spindle
        .overlap_attempt(
            until(),
            false,
            &stop,
            |_| {},
            |_| panic!("stopped request sent"),
        )
        .unwrap();
    assert_eq!(stopped.delivery.sent, 0);
    assert_eq!(spindle.journal.next_sequence(), 2);
    stop.store(false, Ordering::SeqCst);
    assert!(
        spindle
            .overlap_attempt(until(), false, &stop, |_| {}, |_| Delivery::Ack(2))
            .is_err()
    );
    assert_eq!(spindle.journal.acked_through(), 0);
    assert_eq!(spindle.journal.next_sequence(), 3);
    let original = spindle.journal.next_unacked().unwrap().unwrap().1;
    assert!(
        spindle
            .overlap_attempt(
                until(),
                false,
                &stop,
                |_| {},
                |_| panic!("injected worker panic")
            )
            .is_err()
    );
    assert_eq!(spindle.journal.next_unacked().unwrap().unwrap().1, original);
}

#[test]
fn overlap_paused_configuration_and_meter_counters_remain_main_owned() {
    let s = Scratch::new();
    let mut spindle = s.spindle(1024 * 1024);
    spindle.collect_logs().unwrap();
    s.append("must remain unread while paused\n");
    let committed = spindle.meter.committed_bytes();
    let length = spindle.journal.next_unacked().unwrap().unwrap().1.len();
    spindle.applied = Some(RemoteView {
        revision: 1,
        paused: true,
        logs: vec![],
        metric_interval_s: 15,
    });
    let r = spindle
        .overlap_attempt(
            until(),
            false,
            &AtomicBool::new(false),
            |_| {},
            |_| Delivery::Ack(1),
        )
        .unwrap();
    assert!(r.prepared.is_none());
    assert_eq!(spindle.journal.next_sequence(), 2);
    assert_eq!(spindle.meter.committed_bytes(), committed);
    assert_eq!(spindle.meter.delivered_bytes(), length as u64);
}

#[test]
fn overlap_rate_wait_past_deadline_sends_and_prepares_nothing() {
    let s = Scratch::new();
    let mut spindle = s.spindle(1024 * 1024);
    spindle.collect_logs().unwrap();
    s.append("must wait\n");
    spindle.meter = super::super::meter::Meter::new(1, Some(1), 1);
    let _ = spindle.meter.take(100);
    let r = spindle
        .overlap_attempt(
            Instant::now() + Duration::from_millis(1),
            false,
            &AtomicBool::new(false),
            |_| {},
            |_| panic!("rate-limited request sent"),
        )
        .unwrap();
    assert_eq!(r.delivery.sent, 0);
    assert!(r.prepared.is_none());
    assert_eq!(spindle.journal.next_sequence(), 2);
    assert_eq!(spindle.meter.delivered_bytes(), 0);
}
