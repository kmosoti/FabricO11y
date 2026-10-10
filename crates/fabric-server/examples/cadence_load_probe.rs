//! Finite intake-cadence/observer sweep, derived from prefix_load_probe.rs.
//! Preserves its native Store custody and quiet pre/post query fixture.
//! No scheduler ablation: every cell uses the actual production sealer::pass.
//! Build with phase-probe; run only through the coordinated resource launcher.
use fabric_frame::{
    envelope::Batch,
    frame::{FileRef, FrameLog},
};
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
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const LIMIT: u64 = 64 * 1024 * 1024;
const START: u64 = 1_600_000_000_000_000_000;
const STREAMS: usize = 128;
const OFFERS: usize = 512;
const BATCH_BYTES: usize = 8192;

fn encoded(seed: u64, stream: usize, sequence: u64, dense: bool) -> Vec<u8> {
    let (rows, width) = if dense { (128, 32) } else { (2, 2048) };
    let mut logs = ExportLogsServiceRequest {
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
    let mut batch = Batch {
        version: 1,
        node_id: node,
        generation: 1,
        sequence,
        logs: logs.encode_to_vec(),
        ..Default::default()
    };
    // Extend one valid OTLP log body to equalize exact Batch bytes across shapes.
    // This is actual observed payload, preserved by every replay/query oracle.
    assert!(batch.encoded_len() < BATCH_BYTES - 16);
    let original_body = match logs.resource_logs[0].scope_logs[0]
        .log_records
        .last()
        .unwrap()
        .body
        .as_ref()
        .unwrap()
        .value
        .as_ref()
        .unwrap()
    {
        any_value::Value::StringValue(body) => body.clone(),
        _ => unreachable!(),
    };
    let mut padding = BATCH_BYTES - batch.encoded_len();
    loop {
        logs.resource_logs[0].scope_logs[0]
            .log_records
            .last_mut()
            .unwrap()
            .body
            .as_mut()
            .unwrap()
            .value = Some(any_value::Value::StringValue(
            original_body.clone() + &"P".repeat(padding),
        ));
        batch.logs = logs.encode_to_vec();
        let size = batch.encoded_len();
        if size == BATCH_BYTES {
            break;
        }
        if size > BATCH_BYTES {
            padding -= size - BATCH_BYTES;
        } else {
            padding += BATCH_BYTES - size;
        }
    }
    batch.encode_to_vec()
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
fn require_phase_probe() {
    #[cfg(not(feature = "phase-probe"))]
    panic!("cadence fixture requires phase-probe build");
}

fn main() {
    require_phase_probe();
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(
        args.len(),
        6,
        "ROOT balanced3|skewed3|worker1 SEED CADENCE_MS quiet|events|polled"
    );
    assert!(std::env::var_os("FABRIC_SCRATCH_ROOT").is_some());
    let root = PathBuf::from(&args[1]);
    let parent = PathBuf::from(std::env::var("FABRIC_SCRATCH_ROOT").unwrap())
        .canonicalize()
        .unwrap();
    assert_eq!(root.parent().unwrap().canonicalize().unwrap(), parent);
    fs::create_dir(&root).unwrap();
    let shape = args[2].as_str();
    assert!(["balanced3", "skewed3", "worker1"].contains(&shape));
    let seed: u64 = args[3].parse().unwrap();
    assert!([2703204361, 2703204362].contains(&seed));
    let cadence_ms: u64 = args[4].parse().unwrap();
    assert!([0, 2, 5].contains(&cadence_ms));
    let observer = args[5].as_str();
    assert!(["quiet", "events", "polled"].contains(&observer));
    #[cfg(feature = "phase-probe")]
    let events_enabled = observer != "quiet";
    let poll_ms: u64 = if observer == "polled" { 2 } else { 0 };
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
        .map(|i| encoded(seed, i % STREAMS, 4 + (i / STREAMS) as u64, false))
        .collect();
    let stopped = Arc::new(AtomicBool::new(false));
    let done = stopped.clone();
    let monitor_state = state.clone();
    let observer_before = process_facts();
    let observer_setup = Instant::now();
    #[cfg(feature = "phase-probe")]
    if events_enabled {
        fabric_frame::probe::install(|| [0; 4]);
    }
    let observer_setup_ns = observer_setup.elapsed().as_nanos() as u64;
    let before = process_facts();
    let start = Instant::now();
    let anchor_before_ns = start.elapsed().as_nanos() as u64;
    #[cfg(feature = "phase-probe")]
    let schedule_phase = if events_enabled {
        Some(fabric_frame::probe::span("cadence_schedule"))
    } else {
        None
    };
    let anchor_after_ns = start.elapsed().as_nanos() as u64;
    fs::create_dir_all(state.join("segments")).unwrap();
    let monitor = (poll_ms > 0).then(|| std::thread::spawn(move || {
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
            std::thread::sleep(Duration::from_millis(poll_ms));
        }
        events
    }));
    let producer_intake = intake.clone();
    let producer = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let mut pending = Vec::new();
            for (i, batch) in live.into_iter().enumerate() {
                let target = start + Duration::from_millis(i as u64 * cadence_ms);
                if cadence_ms > 0 {
                    tokio::time::sleep_until(tokio::time::Instant::from_std(target)).await;
                } else if i % 32 == 0 {
                    // Let ACK observers run during the finite burst, without imposing a rate.
                    tokio::task::yield_now().await;
                }
                let (tx, rx) = tokio::sync::oneshot::channel();
                let offered_ns = start.elapsed().as_nanos() as u64;
                let n = batch.len();
                producer_intake.submit(submission(batch.clone(), tx));
                pending.push(tokio::spawn(async move {
                    let answer = rx.await.unwrap();
                    let reply_ns = start.elapsed().as_nanos() as u64;
                    (batch, json!({"offer_index":i,"stream":i%STREAMS,"bytes":n,"sequence":4+(i/STREAMS) as u64,"target_ns":i as u64*cadence_ms*1_000_000,
                        "offered_ns":offered_ns,"reply_ns":reply_ns,"latency_ns":reply_ns-offered_ns,"answer":format!("{answer:?}")}), answer)
                }));
            }
            let mut out = Vec::new(); for p in pending { out.push(p.await.unwrap()); } out
        })
    });
    let sealer_begin_ns = start.elapsed().as_nanos() as u64;
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
    let sealer_end_ns = start.elapsed().as_nanos() as u64;
    let offered = producer.join().unwrap();
    stopped.store(true, Ordering::SeqCst);
    let timeline = monitor.map(|h| h.join().unwrap()).unwrap_or_default();
    let schedule_end_ns = start.elapsed().as_nanos() as u64;
    let after = process_facts();
    #[cfg(feature = "phase-probe")]
    drop(schedule_phase);
    #[cfg(feature = "phase-probe")]
    let hooks: Vec<_> = fabric_frame::probe::take()
        .into_iter()
        .map(|r| {
            json!({
                "name":r.name,"thread":format!("{:?}",r.thread),"depth":r.depth,
                "start_ns":r.start_ns,"wall_ns":r.wall_ns
            })
        })
        .collect();
    #[cfg(not(feature = "phase-probe"))]
    let hooks: Vec<Value> = Vec::new();
    fs::write(root.join("hooks.json"), serde_json::to_vec(&hooks).unwrap()).unwrap();
    assert!(labels(&state).unwrap().is_empty());
    let observed_ack_rows: Vec<_> = offered.iter().map(|(_, row, _)| row.clone()).collect();
    fs::write(
        root.join("acks.json"),
        serde_json::to_vec(&observed_ack_rows).unwrap(),
    )
    .unwrap();
    let mut ack_rows = Vec::new();
    for (batch, row, answer) in offered {
        match answer {
            Answer::Ack(sequence)
                if sequence == Batch::decode(batch.as_slice()).unwrap().sequence =>
            {
                expected.push(batch)
            }
            Answer::Unavailable => panic!("unexpected admission failure: preserve complete case"),
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
        .find(|e| Batch::decode(e.batch.as_slice()).unwrap().sequence == 7)
        .expect("some live offer accepted")
        .batch
        .clone();
    assert_eq!(submit(&intake, &last), Answer::Ack(7));
    let mut changed = Batch::decode(last.as_slice()).unwrap();
    changed.collection_gaps.push("different bytes".into());
    assert_eq!(
        submit(&intake, &changed.encode_to_vec()),
        Answer::Conflict(7)
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
    let result = json!({"complete":true,"arm":"production","shape":shape,"seed":seed,"workers":workers,"streams":STREAMS,
        "startup_ns":startup_ns,"sealer_begin_ns":sealer_begin_ns,"sealer_end_ns":sealer_end_ns,"schedule_end_ns":schedule_end_ns,"process_before":before,
        "process_after":after,"poll_ms":poll_ms,"period_ms":cadence_ms,
        "hook_anchor_schedule_ns_bounds":[anchor_before_ns,anchor_after_ns],"batch_bytes":BATCH_BYTES,
        "observer":observer,"observer_setup_ns":observer_setup_ns,"observer_before":observer_before,"initial_labels":initial,"initial_sizes":initial_sizes,
        "accepted":ack_rows.iter().filter(|r| r["answer"].as_str().unwrap().starts_with("Ack(")).count(),"offered":OFFERS,
        "checkpoint_duration":"unmeasured: no labeled checkpoint hook","diagnostic_delay_ms":0,"exact_batch_custody":true,"restart_retry":true});
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
