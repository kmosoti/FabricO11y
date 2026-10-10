//! Native ordered-join ablation. Historical scheduling lives only in this example.
//! Run through tools/resource_group.py; independent query grading is in prefix_load.py.
use fabric_core::retention::SegmentFacts;
use fabric_frame::{
    envelope::Batch,
    frame::{FileRef, FrameLog},
};
use fabric_ports::{Clock, SegmentStore, StoreFailed};
use fabric_server::{
    query::{History, Plan, Query},
    sealer, segment,
    store::{Answer, CommitMode, Entry, Group, Intake, Store, Submission, identify_strand},
};
use opentelemetry_proto::tonic::{
    collector::logs::v1::ExportLogsServiceRequest,
    common::v1::{AnyValue, any_value},
    logs::v1::{LogRecord, ResourceLogs, ScopeLogs},
};
use prost::Message;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const LIMIT: u64 = 64 * 1024 * 1024;
const START: u64 = 1_600_000_000_000_000_000;
const STREAMS: usize = 128;
const OFFERS: usize = 96;
const PERIOD_MS: u64 = 2;
const POLL_MS: u64 = 10;

// Origin: native-build-01 rejected use of the server's private SystemClock
// (E0603, cargo exit101). Keep this example on the public Clock boundary;
// preserve the server adapter's time/error conversion without widening its API.
struct ProbeClock;
impl Clock for ProbeClock {
    fn now_unix_nano(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos() as u64)
    }
}

fn encoded(seed: u64, stream: usize, sequence: u64, dense: bool) -> Vec<u8> {
    let (rows, width) = if dense { (128, 32) } else { (2, 2048) };
    let logs = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: (0..rows)
                    .map(|index| {
                        let prefix = format!("p{seed}-{stream:03}-{sequence}-{index:03} λ ");
                        let body = prefix.clone() + &"R".repeat(width - prefix.len());
                        LogRecord {
                            observed_time_unix_nano: START
                                + sequence * 100_000
                                + stream as u64 * 128
                                + index as u64,
                            body: Some(AnyValue {
                                value: Some(any_value::Value::StringValue(body)),
                            }),
                            ..Default::default()
                        }
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    let mut node = vec![7; 16];
    node[..8].copy_from_slice(&(stream as u64 + 1).to_le_bytes());
    Batch {
        version: 1,
        node_id: node,
        generation: 1,
        sequence,
        logs: logs.encode_to_vec(),
        ..Default::default()
    }
    .encode_to_vec()
}
fn submission(bytes: Vec<u8>, reply: tokio::sync::oneshot::Sender<Answer>) -> Submission {
    let (strand, sequence) = identify_strand(&bytes).unwrap();
    let batch = Batch::decode(bytes.as_slice()).unwrap();
    let stream = u64::from_le_bytes(batch.node_id[..8].try_into().unwrap()) - 1;
    Submission {
        label: format!("fixture-{stream}"),
        strand,
        sequence,
        bytes,
        reply,
    }
}
fn submit(intake: &Intake, bytes: &[u8]) -> Answer {
    let (tx, rx) = tokio::sync::oneshot::channel();
    intake.submit(submission(bytes.to_vec(), tx));
    rx.blocking_recv().unwrap()
}
fn labels(state: &Path) -> io::Result<Vec<u64>> {
    let mut out = Vec::new();
    for e in fs::read_dir(state.join("journal"))? {
        let name = e?.file_name().to_string_lossy().into_owned();
        if let Some(label) = name
            .strip_prefix("sealed-")
            .and_then(|s| s.strip_suffix(".faj"))
        {
            out.push(label.parse().map_err(io::Error::other)?);
        }
    }
    out.sort_unstable();
    Ok(out)
}
// Store has stopped before opening its FrameLog; never publish an active-file subset.
fn close_active(state: &Path) {
    let mut first = None;
    let mut log = FrameLog::open(
        &state.join("journal"),
        LIMIT,
        segment::MAX_GROUP_PAYLOAD,
        |payload, pos| {
            if pos.file == FileRef::Active && first.is_none() {
                first = Some(Group::decode(payload).unwrap().group_sequence);
            }
            Ok(())
        },
    )
    .unwrap();
    log.rotate(first.expect("seed active file must contain actual Groups"))
        .unwrap();
}
fn historical_pass(state: &Path, intake: &Intake, workers: usize, delay_ms: u64) -> io::Result<()> {
    let sealed = labels(state)?;
    let mut published: BTreeSet<u64> = segment::list(state)?.into_iter().map(|(l, _)| l).collect();
    let pending: Vec<_> = sealed
        .iter()
        .copied()
        .filter(|l| !published.contains(l))
        .collect();
    let mut next = 0;
    let reclaim = |published: &BTreeSet<u64>, next: &mut usize| -> io::Result<()> {
        while let Some(&label) = sealed.get(*next) {
            if !published.contains(&label) {
                break;
            }
            intake.reclaim(label)?;
            *next += 1;
        }
        Ok(())
    };
    reclaim(&published, &mut next)?;
    for chunk in pending.chunks(workers.max(1)) {
        let results = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|&label| {
                    scope.spawn(move || {
                        if delay_ms > 0 && label == sealed_label_second(state) {
                            std::thread::sleep(Duration::from_millis(delay_ms));
                        }
                        segment::build_sealed(
                            state,
                            label,
                            &state
                                .join("journal")
                                .join(format!("sealed-{label:020}.faj")),
                        )
                        .map(|_| ())
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .unwrap_or_else(|_| Err(io::Error::other("sealing thread panicked")))
                })
                .collect::<Vec<_>>()
        });
        let mut failed = None;
        for (&label, result) in chunk.iter().zip(results) {
            match result {
                Ok(()) => {
                    published.insert(label);
                }
                Err(e) if failed.is_none() => failed = Some(e),
                Err(_) => {}
            }
        }
        reclaim(&published, &mut next)?;
        if let Some(error) = failed {
            return Err(error);
        }
    }
    // Match the production pass's disabled-retention census and decision cost.
    // Deletion is deliberately unavailable: this experiment retains all history.
    let _ = segment::segments_dir(state)?;
    let removed = fabric_app::retention::apply_retention(
        &mut Retained(state),
        &ProbeClock,
        sealer::Retention {
            max_age_s: u64::MAX,
            max_bytes: u64::MAX,
        },
    )
    .map_err(|StoreFailed(error)| io::Error::other(error))?;
    assert_eq!(removed, 0);
    Ok(())
}
struct Retained<'a>(&'a Path);
impl SegmentStore for Retained<'_> {
    fn sealed(&self) -> Result<Vec<(u64, SegmentFacts)>, StoreFailed> {
        Ok(segment::list(self.0)
            .map_err(|error| StoreFailed(error.to_string()))?
            .into_iter()
            .map(|(label, m)| {
                (
                    label,
                    SegmentFacts {
                        received_max_ns: m.received_max_ns,
                        bytes: m.files.values().map(|file| file.bytes).sum(),
                    },
                )
            })
            .collect())
    }
    fn delete(&mut self, _: u64) -> Result<(), StoreFailed> {
        Err(StoreFailed("experiment must retain all history".into()))
    }
}
fn sealed_label_second(state: &Path) -> u64 {
    labels(state).unwrap().get(1).copied().unwrap_or(u64::MAX)
}
fn replay(state: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    Store::replay(state, LIMIT, |entry| {
        out.push(entry.clone());
        Ok(())
    })
    .unwrap();
    out
}
fn records(root: &Path, stage: &str, entries: &[Entry]) {
    let mut out = fs::File::create(root.join(format!("{stage}-records.jsonl"))).unwrap();
    for e in entries {
        writeln!(
            out,
            "{}",
            json!({"label":e.label,"received_ns":e.received_unix_nano,
        "bytes":b64(&e.batch)})
        )
        .unwrap();
    }
}
fn b64(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let a = chunk[0] as usize;
        let b = chunk.get(1).copied().unwrap_or(0) as usize;
        let c = chunk.get(2).copied().unwrap_or(0) as usize;
        out.push(DIGITS[a >> 2] as char);
        out.push(DIGITS[((a & 3) << 4) | (b >> 4)] as char);
        out.push(if chunk.len() > 1 {
            DIGITS[((b & 15) << 2) | (c >> 6)] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            DIGITS[c & 63] as char
        } else {
            '='
        });
    }
    out
}
fn queries(root: &Path, state: &Path, stage: &str, newest: u64) {
    for plan in [Plan::Scan, Plan::Walk] {
        for (shape, contains) in [("broad", None), ("selective", Some("-001-"))] {
            let mut query =
                json!({"kind":"logs","from_ns":START,"to_ns":START+1_000_000,"limit":1000});
            if let Some(contains) = contains {
                query["contains"] = json!(contains);
            }
            let original = query.clone();
            let history = History::with_plan(state, plan);
            let mut pages = Vec::new();
            loop {
                let parsed: Query = serde_json::from_value(query.clone()).unwrap();
                let answer = history.run(&parsed, newest).unwrap();
                assert_eq!(answer["complete"], true);
                let next = answer["next_page"].clone();
                pages.push(answer);
                assert!(pages.len() <= 64);
                if next.is_null() {
                    break;
                }
                query["page"] = next;
            }
            let label = format!("{stage}-{plan:?}-{shape}");
            fs::write(
                root.join(format!("{label}-query.json")),
                original.to_string(),
            )
            .unwrap();
            fs::write(
                root.join(format!("{label}-answer.json")),
                serde_json::to_vec(&pages).unwrap(),
            )
            .unwrap();
        }
    }
}
fn process_facts() -> Value {
    let status = fs::read_to_string("/proc/self/status").unwrap();
    let values: Vec<_> = status
        .lines()
        .filter(|l| l.starts_with("VmRSS:") || l.starts_with("VmHWM:"))
        .map(str::to_owned)
        .collect();
    let stat = fs::read_to_string("/proc/self/stat").unwrap();
    let fields: Vec<_> = stat
        .rsplit_once(") ")
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    json!({"rss_status":values,"utime_ticks":fields[11].parse::<u64>().unwrap(),"stime_ticks":fields[12].parse::<u64>().unwrap(),"clock":"Linux /proc process ticks; CLK_TCK recorded by driver"})
}
fn bytes(path: &Path) -> u64 {
    fs::read_dir(path)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            let kind = e.file_type().unwrap();
            assert!(!kind.is_symlink());
            if kind.is_dir() {
                bytes(&e.path())
            } else {
                e.metadata().unwrap().len()
            }
        })
        .sum()
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(
        args.len(),
        5,
        "ROOT baseline|candidate balanced3|skewed3|worker1 SEED"
    );
    assert!(std::env::var_os("FABRIC_SCRATCH_ROOT").is_some());
    let root = PathBuf::from(&args[1]);
    let parent = PathBuf::from(std::env::var("FABRIC_SCRATCH_ROOT").unwrap())
        .canonicalize()
        .unwrap();
    assert_eq!(root.parent().unwrap().canonicalize().unwrap(), parent);
    fs::create_dir(&root).unwrap();
    let arm = args[2].as_str();
    assert!(["baseline", "candidate"].contains(&arm));
    let shape = args[3].as_str();
    assert!(["balanced3", "skewed3", "worker1"].contains(&shape));
    let seed: u64 = args[4].parse().unwrap();
    assert!([2703204353, 2703204354, 2703204355].contains(&seed));
    // Only historical scheduling supports optional deliberate delay. It is never a paired production result.
    let delay_ms: u64 =
        std::env::var("FABRIC_PREFIX_DIAGNOSTIC_DELAY_MS").map_or(0, |v| v.parse().unwrap());
    assert!(delay_ms <= 1000 && (delay_ms == 0 || arm == "baseline"));
    let state = root.join("state");
    let startup = Instant::now();
    let mut expected = Vec::new();
    for sequence in 1..=3 {
        let (intake, handle) = Store::open_with(&state, LIMIT, LIMIT, CommitMode::INDIVIDUAL)
            .unwrap()
            .spawn_joinable()
            .unwrap();
        for stream in 0..STREAMS {
            let batch = encoded(
                seed,
                stream,
                sequence,
                shape != "balanced3" && sequence == 2,
            );
            assert_eq!(submit(&intake, &batch), Answer::Ack(sequence));
            expected.push(batch);
        }
        drop(intake);
        handle.join().unwrap();
        close_active(&state);
    }
    let startup_ns = startup.elapsed().as_nanos() as u64;
    let initial = labels(&state).unwrap();
    assert_eq!(initial.len(), 3);
    let initial_sizes: Vec<_> = initial
        .iter()
        .map(|label| {
            (
                *label,
                fs::metadata(
                    state
                        .join("journal")
                        .join(format!("sealed-{label:020}.faj")),
                )
                .unwrap()
                .len(),
            )
        })
        .collect();
    let seed_entries = replay(&state);
    assert_eq!(
        seed_entries
            .iter()
            .map(|e| e.batch.clone())
            .collect::<Vec<_>>(),
        expected
    );
    records(&root, "pre", &seed_entries);
    queries(&root, &state, "pre", STREAMS as u64 * 3);
    let (intake, commit) = Store::open_with(&state, LIMIT, LIMIT, CommitMode::GROUPED)
        .unwrap()
        .spawn_joinable()
        .unwrap();
    let workers = if shape == "worker1" { 1 } else { 3 };
    let live: Vec<_> = (0..OFFERS)
        .map(|stream| encoded(seed, stream, 4, false))
        .collect();
    let stopped = Arc::new(AtomicBool::new(false));
    let done = stopped.clone();
    let monitor_state = state.clone();
    let start = Instant::now();
    let before = process_facts();
    fs::create_dir_all(state.join("segments")).unwrap();
    let monitor = std::thread::spawn(move || {
        let mut events = Vec::new();
        loop {
            let ns_begin = start.elapsed().as_nanos() as u64;
            let journals: Vec<_> = labels(&monitor_state)
                .unwrap()
                .into_iter()
                .map(|label| {
                    let path = monitor_state
                        .join("journal")
                        .join(format!("sealed-{label:020}.faj"));
                    (label, fs::metadata(path).map(|m| m.len()).ok())
                })
                .collect();
            // Filename observation has polling uncertainty; do not validate/hash Segments during the timed schedule.
            let segments: Vec<_> = fs::read_dir(monitor_state.join("segments"))
                .unwrap()
                .filter_map(|e| {
                    let name = e.unwrap().file_name().to_string_lossy().into_owned();
                    name.strip_prefix("seg-")
                        .and_then(|s| s.parse::<u64>().ok())
                })
                .collect();
            events.push(json!({"ns_begin":ns_begin,"ns":start.elapsed().as_nanos() as u64,"journals":journals,"segments":segments,"process":process_facts()}));
            if done.load(Ordering::SeqCst) {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(18));
            std::thread::sleep(Duration::from_millis(POLL_MS));
        }
        events
    });
    let producer_intake = intake.clone();
    let producer = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let mut pending = Vec::new();
            for (i, batch) in live.into_iter().enumerate() {
                let target = start + Duration::from_millis(i as u64 * PERIOD_MS);
                tokio::time::sleep_until(tokio::time::Instant::from_std(target)).await;
                let (tx, rx) = tokio::sync::oneshot::channel();
                let offered_ns = start.elapsed().as_nanos() as u64;
                let n = batch.len();
                producer_intake.submit(submission(batch.clone(), tx));
                pending.push(tokio::spawn(async move {
                    let answer = rx.await.unwrap();
                    let reply_ns = start.elapsed().as_nanos() as u64;
                    (batch, json!({"stream":i,"bytes":n,"target_ns":i as u64*PERIOD_MS*1_000_000,
                        "offered_ns":offered_ns,"reply_ns":reply_ns,"latency_ns":reply_ns-offered_ns,"answer":format!("{answer:?}")}), answer)
                }));
            }
            let mut out = Vec::new(); for p in pending { out.push(p.await.unwrap()); } out
        })
    });
    if arm == "candidate" {
        sealer::pass(
            &state,
            &intake,
            sealer::Retention {
                max_age_s: u64::MAX,
                max_bytes: u64::MAX,
            },
            workers,
        )
        .unwrap();
    } else {
        historical_pass(&state, &intake, workers, delay_ms).unwrap();
    }
    let sealer_end_ns = start.elapsed().as_nanos() as u64;
    let offered = producer.join().unwrap();
    stopped.store(true, Ordering::SeqCst);
    let timeline = monitor.join().unwrap();
    let schedule_end_ns = start.elapsed().as_nanos() as u64;
    let after = process_facts();
    assert!(labels(&state).unwrap().is_empty());
    let mut ack_rows = Vec::new();
    for (batch, row, answer) in offered {
        match answer {
            Answer::Ack(4) => expected.push(batch),
            Answer::Unavailable => {}
            other => panic!("unexpected answer {other:?}"),
        }
        ack_rows.push(row);
    }
    drop(intake);
    commit.join().unwrap();
    let mut actual = replay(&state);
    let mut got: Vec<_> = actual.iter().map(|e| e.batch.clone()).collect();
    expected.sort();
    got.sort();
    assert_eq!(got, expected, "exact encoded Batch multiset custody");
    records(&root, "post", &actual);
    let (intake, commit) = Store::open_with(&state, LIMIT, LIMIT, CommitMode::GROUPED)
        .unwrap()
        .spawn_joinable()
        .unwrap();
    // Store sends replies before updating its published group counter. Reopen
    // only after the original commit thread has drained/joined; this counter is
    // initialized from exact durable replay, without relying on observer timing.
    let newest = intake.committed_group();
    queries(&root, &state, "post", newest);
    let last = actual
        .iter()
        .rev()
        .find(|e| Batch::decode(e.batch.as_slice()).unwrap().sequence == 4)
        .expect("some live offer accepted")
        .batch
        .clone();
    assert_eq!(submit(&intake, &last), Answer::Ack(4));
    let mut changed = Batch::decode(last.as_slice()).unwrap();
    changed.collection_gaps.push("different bytes".into());
    assert_eq!(
        submit(&intake, &changed.encode_to_vec()),
        Answer::Conflict(4)
    );
    drop(intake);
    commit.join().unwrap();
    let recovered = replay(&state);
    assert_eq!(recovered, actual);
    actual.clear();
    fs::write(
        root.join("timeline.json"),
        serde_json::to_vec(&timeline).unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("acks.json"),
        serde_json::to_vec(&ack_rows).unwrap(),
    )
    .unwrap();
    let result = json!({"complete":true,"arm":arm,"shape":shape,"seed":seed,"workers":workers,"streams":STREAMS,
        "startup_ns":startup_ns,"sealer_end_ns":sealer_end_ns,"schedule_end_ns":schedule_end_ns,"process_before":before,
        "process_after":after,"poll_ms":POLL_MS,"period_ms":PERIOD_MS,"initial_labels":initial,"initial_sizes":initial_sizes,
        "accepted":ack_rows.iter().filter(|r| r["answer"]=="Ack(4)").count(),"offered":OFFERS,
        "checkpoint_duration":"unmeasured","diagnostic_delay_ms":delay_ms,"exact_batch_custody":true,"restart_retry":true});
    fs::write(root.join("result.json"), result.to_string()).unwrap();
    assert!(
        bytes(&state) <= LIMIT,
        "state cap exceeded; preserve evidence"
    );
    assert!(
        bytes(&root) <= 128 * 1024 * 1024,
        "raw cap exceeded; preserve evidence"
    );
    println!("{result}");
}
