//! Exact legacy encoding and durable collection ownership controls.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

// Frozen borrowed implementation from before ownership transfer. Keep this
// independent of owned_log_record so a shared construction bug cannot hide.
fn legacy_record(line: &log_source::Line, now: u64) -> LogRecord {
    LogRecord {
        observed_time_unix_nano: now,
        body: Some(AnyValue {
            value: Some(any_value::Value::StringValue(line.body.clone())),
        }),
        attributes: vec![
            attr("log.file.path", &line.path),
            attr("log.file.device", line.device.to_string()),
            attr("log.file.inode", line.inode.to_string()),
            attr("log.file.offset.start", line.start.to_string()),
            attr("log.file.offset.end", line.end.to_string()),
        ],
        ..Default::default()
    }
}
fn legacy_logs(lines: &[log_source::Line], now: u64) -> Vec<u8> {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(resource("fixture-host", "fixture-boot")),
            scope_logs: vec![ScopeLogs {
                log_records: lines.iter().map(|line| legacy_record(line, now)).collect(),
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec()
}
fn line(body: &str, path: &str, start: u64) -> log_source::Line {
    log_source::Line {
        body: body.into(),
        path: path.into(),
        device: u64::MAX,
        inode: 9_007_199_254_740_993,
        start,
        end: start + body.len() as u64 + 1,
    }
}
fn string_value(value: &AnyValue) -> &str {
    match value.value.as_ref().unwrap() {
        any_value::Value::StringValue(text) => text,
        _ => panic!("string required"),
    }
}
#[test]
fn ownership_log_encoding_matches_legacy_exact_bytes_and_fields() {
    let fixture = || {
        vec![
            line("", "/empty.log", 0),
            line("雪🙂\tquote\"", "/日志/α.log", 17),
            line("abc", "/numeric.log", 9_007_199_254_740_993),
        ]
    };
    let now = 1_800_000_000_000_000_001;
    let expected = legacy_logs(&fixture(), now);
    let actual = encoded_logs(fixture(), "fixture-host", "fixture-boot", now);
    assert_eq!(actual, expected);
    let request = ExportLogsServiceRequest::decode(actual.as_slice()).unwrap();
    let records = &request.resource_logs[0].scope_logs[0].log_records;
    assert_eq!(records.len(), 3);
    for (record, source) in records.iter().zip(fixture()) {
        assert_eq!(record.observed_time_unix_nano, now);
        assert_eq!(string_value(record.body.as_ref().unwrap()), source.body);
        let fields: Vec<_> = record
            .attributes
            .iter()
            .map(|a| {
                (
                    a.key.as_str(),
                    string_value(a.value.as_ref().unwrap()).to_owned(),
                )
            })
            .collect();
        assert_eq!(
            fields,
            vec![
                ("log.file.path", source.path),
                ("log.file.device", u64::MAX.to_string()),
                ("log.file.inode", 9_007_199_254_740_993_u64.to_string()),
                ("log.file.offset.start", source.start.to_string()),
                ("log.file.offset.end", source.end.to_string())
            ]
        );
    }
}
#[test]
fn ownership_log_strings_move_and_legacy_clone_control_detects_copies() {
    let source = line("owned body 雪", "/owned/path.log", 1);
    let body = source.body.as_ptr();
    let path = source.path.as_ptr();
    let baseline = legacy_record(&source, 2);
    assert_ne!(string_value(baseline.body.as_ref().unwrap()).as_ptr(), body);
    assert_ne!(
        string_value(baseline.attributes[0].value.as_ref().unwrap()).as_ptr(),
        path
    );
    let record = owned_log_record(source, 2);
    assert_eq!(string_value(record.body.as_ref().unwrap()).as_ptr(), body);
    assert_eq!(
        string_value(record.attributes[0].value.as_ref().unwrap()).as_ptr(),
        path
    );
    assert_eq!(record, baseline);
}
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root = PathBuf::from(
            std::env::var_os("FABRIC_SCRATCH_ROOT").expect("contained data-drive scratch"),
        );
        let path = root.join(format!(
            "runtime-ownership-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn spindle(&self, cap: u64, valid_host: bool) -> Spindle {
        let root = &self.0;
        std::fs::write(root.join("input.log"), "").unwrap();
        if valid_host {
            std::fs::write(root.join("stat"), "cpu 10 0 5 20 0 0 0 0 0 0\nbtime 1000\n").unwrap();
            std::fs::write(
                root.join("meminfo"),
                "MemTotal: 1000 kB\nMemAvailable: 500 kB\n",
            )
            .unwrap();
            std::fs::write(root.join("diskstats"), "8 0 sda 1 0 4 0 1 0 8 0\n").unwrap();
            std::fs::write(root.join("netdev"), "Inter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast |bytes packets errs drop fifo colls carrier compressed\neth0: 10 0 0 0 0 0 0 0 20 0 0 0 0 0 0 0\n").unwrap();
            std::fs::write(
                root.join("boot_id"),
                "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa\n",
            )
            .unwrap();
            std::fs::write(root.join("hostname"), "fixture-host\n").unwrap();
        }
        let mut spindle = Spindle::open_with_paths(
            Config {
                spool: root.join("spool"),
                logs: vec![root.join("input.log")],
                interval_s: 15,
                spool_bytes: cap,
                server: None,
                traces_listen: None,
                max_output_bytes_per_s: None,
            },
            Paths {
                proc_stat: root.join("stat"),
                proc_meminfo: root.join("meminfo"),
                proc_diskstats: root.join("diskstats"),
                proc_net_dev: root.join("netdev"),
                boot_id: root.join("boot_id"),
                hostname: root.join("hostname"),
                filesystem: root.clone(),
            },
        )
        .unwrap();
        spindle
            .history
            .insert("prior-counter-series".into(), (Value::Int(91), 123));
        spindle
            .history
            .insert("prior-float-series".into(), (Value::Double(0.25), 456));
        spindle
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn history_snapshot(history: &History) -> Vec<(String, (u8, u64), u64, usize)> {
    history
        .iter()
        .map(|(key, (value, start))| {
            (
                key.clone(),
                match value {
                    Value::Int(n) => (0, *n),
                    Value::Double(n) => (1, n.to_bits()),
                },
                *start,
                key.as_ptr() as usize,
            )
        })
        .collect()
}
#[test]
fn ownership_quiet_and_log_only_collection_preserve_history_allocations() {
    let scratch = Scratch::new();
    let mut spindle = scratch.spindle(1024 * 1024, false);
    let before = history_snapshot(&spindle.history);
    assert!(spindle.collect_logs().unwrap().is_none());
    assert_eq!(history_snapshot(&spindle.history), before);
    std::fs::write(scratch.0.join("input.log"), "new line\n").unwrap();
    assert_eq!(spindle.collect_logs().unwrap().unwrap().log_records, 1);
    assert_eq!(history_snapshot(&spindle.history), before);
    assert_eq!(spindle.cursors.values().next().unwrap().offset, 9);
}
#[test]
fn ownership_host_failure_keeps_history_after_durable_gap_commit() {
    let scratch = Scratch::new();
    let mut spindle = scratch.spindle(1024 * 1024, false);
    let before = history_snapshot(&spindle.history);
    let cycle = spindle.collect_once().unwrap();
    assert!(cycle.gaps > 0);
    assert_eq!(cycle.metric_points, 0);
    assert_eq!(history_snapshot(&spindle.history), before);
    assert_eq!(spindle.journal.next_sequence(), 2);
}
#[test]
fn ownership_failed_append_keeps_history_and_source_cursors() {
    let scratch = Scratch::new();
    let mut spindle = scratch.spindle(8192, true);
    let before = history_snapshot(&spindle.history);
    let cursors = spindle.cursors.clone();
    std::fs::write(
        scratch.0.join("input.log"),
        format!(
            "{}\n{}\n{}\n",
            "x".repeat(4000),
            "y".repeat(4000),
            "z".repeat(4000)
        ),
    )
    .unwrap();
    // Use valid metrics to construct a prospective replacement history; fill the
    // Spool first so even a byte-budgeted candidate is refused on durable append.
    while spindle
        .journal
        .append_owned(Batch {
            version: 1,
            node_id: vec![],
            generation: 0,
            sequence: 0,
            metrics: vec![],
            logs: vec![],
            traces: vec![],
            cursors: vec![],
            collection_gaps: vec!["f".repeat(MAX_GAP_BYTES)],
        })
        .is_ok()
    {}
    assert!(spindle.collect_once().is_err());
    assert_eq!(history_snapshot(&spindle.history), before);
    assert_eq!(spindle.cursors, cursors);
    assert!(read_unknown(&spindle.config.spool).is_some());
}

#[test]
fn ownership_successful_metrics_replace_history_only_after_commit() {
    let scratch = Scratch::new();
    let mut spindle = scratch.spindle(1024 * 1024, true);
    std::fs::write(scratch.0.join("input.log"), "committed\n").unwrap();
    let cycle = spindle.collect_once().unwrap();
    assert!(cycle.metric_points > 0);
    assert!(!spindle.history.is_empty());
    assert!(!spindle.history.contains_key("prior-counter-series"));
    assert!(!spindle.history.contains_key("prior-float-series"));
    assert_eq!(cycle.log_records, 1);
    assert_eq!(spindle.cursors.values().next().unwrap().offset, 10);
    assert_eq!(
        spindle.journal.next_unacked().unwrap().unwrap().0,
        cycle.batch_sequence
    );
}

#[test]
#[ignore = "registered contained encoding diagnostic; no service performance claim"]
fn ownership_encoding_finite_pairs() {
    for body_bytes in [128, 4000] {
        let repeats = if body_bytes == 128 { 100 } else { 20 };
        let line_count = if body_bytes == 128 { 1024 } else { 192 };
        for pair in 0..3 {
            let fixture = || {
                (0..line_count)
                    .map(|i| {
                        line(
                            &"x".repeat(body_bytes),
                            "fixture.log",
                            i as u64 * (body_bytes + 1) as u64,
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let borrowed_inputs: Vec<_> = (0..repeats).map(|_| fixture()).collect();
            let owned_inputs: Vec<_> = (0..repeats).map(|_| fixture()).collect();
            let borrowed = || {
                let start = Instant::now();
                let results: Vec<_> = borrowed_inputs
                    .iter()
                    .map(|lines| legacy_logs(lines, 123))
                    .collect();
                (start.elapsed().as_nanos(), results)
            };
            let owned = || {
                let start = Instant::now();
                let results: Vec<_> = owned_inputs
                    .into_iter()
                    .map(|lines| encoded_logs(lines, "fixture-host", "fixture-boot", 123))
                    .collect();
                (start.elapsed().as_nanos(), results)
            };
            let ((borrowed_ns, baseline), (owned_ns, candidate)) = if pair % 2 == 0 {
                (borrowed(), owned())
            } else {
                let candidate = owned();
                (borrowed(), candidate)
            };
            assert_eq!(baseline, candidate); // All exact-byte checks outside timers.
            assert!(candidate[0].len() <= LOG_BODY_BUDGET);
            println!(
                "{}",
                serde_json::json!({"fixture":"log-encoding-only", "pair":pair + 1,
                "lines":line_count,"body_bytes":body_bytes,"repeats":repeats,
                "borrowed_ns":borrowed_ns,"owned_ns":owned_ns,"exact_bytes":true,
                "encoded_bytes":candidate[0].len()})
            );
        }
    }
}
