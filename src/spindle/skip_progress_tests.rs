//! Oversized-line skip progress regression: catalog-log-progress-repro-01
//! retained the second-pass None/cursor1MiB/ACK1 failure before correction.
//! docs/PRODUCT-CONTRACT.md cursor/custody rules require source movement to
//! follow durable Batch commit. The original desired trajectory stays unchanged.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
const MIB: u64 = 1024 * 1024;
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root = PathBuf::from(
            std::env::var_os("FABRIC_SCRATCH_ROOT").expect("contained data-drive scratch"),
        );
        let path = root.join(format!(
            "skip-progress-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn paths(&self) -> Paths {
        Paths {
            proc_stat: self.0.join("stat"),
            proc_meminfo: self.0.join("meminfo"),
            proc_diskstats: self.0.join("diskstats"),
            proc_net_dev: self.0.join("netdev"),
            boot_id: self.0.join("boot_id"),
            hostname: self.0.join("hostname"),
            filesystem: self.0.clone(),
        }
    }
    fn config(&self) -> Config {
        Config {
            spool: self.0.join("spool"),
            logs: vec![self.0.join("input.log")],
            interval_s: 3600,
            spool_bytes: 1024 * 1024,
            server: None,
            traces_listen: None,
            max_output_bytes_per_s: None,
        }
    }
    fn prepare(&self) -> Vec<u8> {
        std::fs::write(
            self.0.join("stat"),
            "cpu 10 0 5 20 0 0 0 0 0 0\nbtime 1000\n",
        )
        .unwrap();
        std::fs::write(
            self.0.join("meminfo"),
            "MemTotal: 1000 kB\nMemAvailable: 500 kB\n",
        )
        .unwrap();
        std::fs::write(self.0.join("diskstats"), "8 0 sda 1 0 4 0 1 0 8 0\n").unwrap();
        std::fs::write(self.0.join("netdev"), "Inter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast |bytes packets errs drop fifo colls carrier compressed\neth0: 10 0 0 0 0 0 0 0 20 0 0 0 0 0 0 0\n").unwrap();
        std::fs::write(
            self.0.join("boot_id"),
            "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa\n",
        )
        .unwrap();
        std::fs::write(self.0.join("hostname"), "fixture-host\n").unwrap();
        let mut bytes = vec![b'X'; (3 * MIB + 10) as usize];
        bytes.extend_from_slice(b"\nnormal\n");
        std::fs::write(self.0.join("input.log"), &bytes).unwrap();
        bytes
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn save_trace(scratch: &Scratch, trace: &serde_json::Value) {
    let path = scratch.0.join("trace.json");
    let mut file = File::create(path).unwrap();
    file.write_all(serde_json::to_string_pretty(trace).unwrap().as_bytes())
        .unwrap();
    file.sync_all().unwrap();
    println!(
        "{}",
        serde_json::json!({"skip_progress_trace":scratch.0.join("trace.json")})
    );
    if trace.get("final").is_some() {
        println!("{}", serde_json::json!({"skip_progress_final":trace}));
    }
}

#[test]
fn skip_only_passes_commit_forward_progress_without_duplicate_gap_or_suffix() {
    let scratch = Scratch::new();
    let input = scratch.prepare();
    let mut trace = serde_json::json!({"origin":"prospective runtime skip-only progress regression",
        "recipe":{"repeated_byte":"X","repeat":3*MIB+10,"suffix":"\nnormal\n"},
        "input_bytes":input.len(),"input_sha256":format!("{:x}",sha2::Sha256::digest(&input)),
        "expected_offsets":[MIB,2*MIB,3*MIB,3145746],"passes":[]});
    save_trace(&scratch, &trace);
    let cfg = scratch.config();
    let mut spindle = Spindle::open_with_paths(cfg.clone(), scratch.paths()).unwrap();
    let expected_offsets = [MIB, 2 * MIB, 3 * MIB, input.len() as u64];
    for (index, expected_offset) in expected_offsets.into_iter().enumerate() {
        let cycle = spindle.collect_logs().unwrap();
        let cursor = spindle.cursors.values().next().cloned();
        trace["passes"].as_array_mut().unwrap().push(serde_json::json!({
            "pass":index+1,"returned_cycle":cycle.as_ref().map(|c|serde_json::json!({
                "sequence":c.batch_sequence,"logs":c.log_records,"metrics":c.metric_points,
                "gaps":c.gaps,"backlog":c.log_backlog_bytes})),
            "cursor":cursor.as_ref().map(|c|serde_json::json!({"offset":c.offset,"skipping_oversize":c.skipping_oversize})),
            "next_sequence":spindle.journal.next_sequence(),"acked":spindle.acked_through()}));
        save_trace(&scratch, &trace); // Record actual behavior BEFORE desired assertion.
        let cycle = cycle.expect("skip-only pass must durably commit forward cursor progress");
        assert_eq!(cycle.batch_sequence, (index + 1) as u64);
        assert_eq!(cursor.as_ref().unwrap().offset, expected_offset);
        assert_eq!(cursor.as_ref().unwrap().skipping_oversize, index < 3);
        if index == 0 {
            assert_eq!((cycle.log_records, cycle.gaps), (0, 1));
        }
        if index == 3 {
            assert_eq!(cycle.log_records, 1);
        }
        spindle.journal.record_ack(cycle.batch_sequence).unwrap();
        assert_eq!(spindle.acked_through(), cycle.batch_sequence);
        assert!(spindle.journal.next_unacked().unwrap().is_none());
        trace["passes"][index]["acked_after"] = serde_json::json!(spindle.acked_through());
        if index == 1 {
            drop(spindle);
            spindle = Spindle::open_with_paths(cfg.clone(), scratch.paths()).unwrap();
            assert_eq!(spindle.cursors.values().next().unwrap().offset, 2 * MIB);
            assert!(spindle.cursors.values().next().unwrap().skipping_oversize);
            assert_eq!(spindle.acked_through(), 2);
            trace["reopened_after_pass"] = serde_json::json!(2);
        }
        save_trace(&scratch, &trace);
    }
    let mut gaps = Vec::new();
    let mut rows = Vec::new();
    spindle
        .journal
        .replay(|batch| {
            gaps.extend(batch.collection_gaps);
            if !batch.logs.is_empty() {
                for resource in ExportLogsServiceRequest::decode(batch.logs.as_slice())
                    .unwrap()
                    .resource_logs
                {
                    for scope in resource.scope_logs {
                        rows.extend(scope.log_records);
                    }
                }
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(gaps.len(), 1);
    assert!(gaps[0].starts_with("oversize log line skipped:"));
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].body.as_ref().unwrap().value,
        Some(any_value::Value::StringValue("normal".into()))
    );
    let fields: BTreeMap<_, _> = rows[0]
        .attributes
        .iter()
        .map(|a| {
            let Some(any_value::Value::StringValue(value)) =
                a.value.as_ref().unwrap().value.as_ref()
            else {
                panic!("string attribute")
            };
            (a.key.as_str(), value.as_str())
        })
        .collect();
    assert_eq!(fields["log.file.offset.start"], "3145739");
    assert_eq!(fields["log.file.offset.end"], "3145746");
    trace["final"] =
        serde_json::json!({"logs":rows.len(),"gaps":gaps,"acked":spindle.acked_through()});
    save_trace(&scratch, &trace);
}

fn history_state(spindle: &Spindle) -> Vec<(String, (u8, u64), u64, usize)> {
    spindle
        .history
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
fn start_skip(scratch: &Scratch, cfg: Config) -> Spindle {
    let mut spindle = Spindle::open_with_paths(cfg, scratch.paths()).unwrap();
    let first = spindle.collect_logs().unwrap().unwrap();
    assert_eq!((first.log_records, first.gaps), (0, 1));
    assert_eq!(spindle.cursors.values().next().unwrap().offset, MIB);
    spindle.journal.record_ack(first.batch_sequence).unwrap();
    spindle
}

#[test]
fn skip_progress_full_spool_refuses_cursor_then_reopen_retry_commits_it() {
    let scratch = Scratch::new();
    scratch.prepare();
    let mut cfg = scratch.config();
    cfg.spool_bytes = 8192;
    let mut spindle = start_skip(&scratch, cfg.clone());
    spindle
        .history
        .insert("retained-counter".into(), (Value::Int(77), 123));
    let history = history_state(&spindle);
    let cursors = spindle.cursors.clone();
    loop {
        let filler = Batch {
            version: 1,
            node_id: vec![],
            generation: 0,
            sequence: 0,
            metrics: vec![],
            logs: vec![],
            traces: vec![],
            cursors: vec![],
            collection_gaps: vec!["capacity fixture".repeat(16)],
        };
        if spindle.journal.append_owned(filler).is_err() {
            break;
        }
    }
    let next = spindle.journal.next_sequence();
    assert!(spindle.collect_logs().is_err());
    assert_eq!(spindle.cursors, cursors);
    assert_eq!(history_state(&spindle), history);
    assert_eq!(spindle.journal.next_sequence(), next);
    assert!(read_unknown(&spindle.config.spool).is_some());
    drop(spindle);
    cfg.spool_bytes = 1024 * 1024; // Explicit allowed test cap, same durable identity.
    let mut spindle = Spindle::open_with_paths(cfg, scratch.paths()).unwrap();
    assert_eq!(spindle.cursors, cursors);
    let retry = spindle.collect_logs().unwrap().unwrap();
    assert_eq!(retry.batch_sequence, next);
    assert_eq!(spindle.cursors.values().next().unwrap().offset, 2 * MIB);
    assert!(spindle.cursors.values().next().unwrap().skipping_oversize);
    assert_eq!(retry.log_records, 0);
    assert!(retry.gaps > 0); // Actual capacity unknown interval, not a new skip gap.
    assert!(read_unknown(&spindle.config.spool).is_none());
}

#[test]
fn skip_progress_host_failure_commits_existing_failure_gap_and_preserves_history() {
    let scratch = Scratch::new();
    scratch.prepare();
    let mut spindle = start_skip(&scratch, scratch.config());
    spindle
        .history
        .insert("retained-counter".into(), (Value::Int(77), 123));
    let history = history_state(&spindle);
    std::fs::remove_file(scratch.0.join("stat")).unwrap();
    let second = spindle.collect_logs().unwrap().unwrap();
    assert_eq!(
        (second.log_records, second.metric_points, second.gaps),
        (0, 0, 1)
    );
    assert_eq!(spindle.cursors.values().next().unwrap().offset, 2 * MIB);
    assert_eq!(history_state(&spindle), history);
    let mut committed = Vec::new();
    spindle
        .journal
        .replay(|batch| {
            committed.push(batch);
            Ok(())
        })
        .unwrap();
    assert!(committed[1].collection_gaps[0].starts_with("host metrics unavailable:"));
    assert!(committed[1].metrics.is_empty());
}

#[test]
fn skip_progress_true_eof_and_ordinary_quiet_do_not_sample_or_commit() {
    for skipping in [false, true] {
        let scratch = Scratch::new();
        scratch.prepare();
        std::fs::write(
            scratch.0.join("input.log"),
            if skipping {
                vec![b'X'; MIB as usize]
            } else {
                vec![]
            },
        )
        .unwrap();
        let mut spindle = Spindle::open_with_paths(scratch.config(), scratch.paths()).unwrap();
        if skipping {
            let first = spindle.collect_logs().unwrap().unwrap();
            spindle.journal.record_ack(first.batch_sequence).unwrap();
        }
        std::fs::remove_file(scratch.0.join("stat")).unwrap();
        let before = spindle.journal.next_sequence();
        let cursors = spindle.cursors.clone();
        assert!(spindle.collect_logs().unwrap().is_none());
        assert_eq!(spindle.journal.next_sequence(), before);
        assert_eq!(spindle.cursors, cursors);
    }
}

#[test]
fn skip_progress_closing_newline_commits_without_consuming_incomplete_suffix() {
    let scratch = Scratch::new();
    scratch.prepare();
    let mut input = vec![b'X'; (MIB + 10) as usize];
    input.extend_from_slice(b"\npartial");
    std::fs::write(scratch.0.join("input.log"), &input).unwrap();
    let mut spindle = start_skip(&scratch, scratch.config());
    let second = spindle.collect_logs().unwrap().unwrap();
    assert_eq!(second.log_records, 0);
    assert_eq!(second.gaps, 0);
    assert!(second.metric_points > 0);
    let cursor = spindle.cursors.values().next().unwrap();
    assert_eq!(cursor.offset, MIB + 11);
    assert!(!cursor.skipping_oversize);
    spindle.journal.record_ack(second.batch_sequence).unwrap();
    assert!(spindle.collect_logs().unwrap().is_none());
    assert_eq!(spindle.cursors.values().next().unwrap().offset, MIB + 11);
    OpenOptions::new()
        .append(true)
        .open(scratch.0.join("input.log"))
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    let third = spindle.collect_logs().unwrap().unwrap();
    assert_eq!((third.log_records, third.gaps), (1, 0));
    assert_eq!(spindle.cursors.values().next().unwrap().offset, MIB + 19);
    let mut batches = Vec::new();
    spindle
        .journal
        .replay(|batch| {
            batches.push(batch);
            Ok(())
        })
        .unwrap();
    let request = ExportLogsServiceRequest::decode(batches[2].logs.as_slice()).unwrap();
    let record = &request.resource_logs[0].scope_logs[0].log_records[0];
    assert_eq!(
        record.body.as_ref().unwrap().value,
        Some(any_value::Value::StringValue("partial".into()))
    );
    let fields: BTreeMap<_, _> = record
        .attributes
        .iter()
        .map(|a| {
            let Some(any_value::Value::StringValue(v)) = a.value.as_ref().unwrap().value.as_ref()
            else {
                panic!("string attribute")
            };
            (a.key.as_str(), v.as_str())
        })
        .collect();
    assert_eq!(fields["log.file.offset.start"], (MIB + 11).to_string());
    assert_eq!(fields["log.file.offset.end"], (MIB + 19).to_string());
}

fn stable_quiet_read(
    path: &std::path::Path,
    prior: Option<&Cursor>,
    budget: usize,
    per_line: usize,
) -> io::Result<log_source::ReadResult> {
    // Actual ext4 fixture read and prefix checks, followed by the injected
    // successful namespace observation. No Btrfs or reboot claim from this seam.
    let mut read = log_source::read_lines_costed(path, prior, budget, per_line)?;
    read.cursor.btrfs_identity = Some(fabric_frame::envelope::BtrfsIdentity {
        uuid: vec![9; 16],
        subvolume_id: 256,
    });
    Ok(read)
}

fn legacy_quiet_spindle(scratch: &Scratch, config: Config) -> Spindle {
    scratch.prepare();
    std::fs::write(scratch.0.join("input.log"), b"one\ntwo\nthree\n").unwrap();
    let mut spindle = Spindle::open_with_paths(config, scratch.paths()).unwrap();
    let first = spindle.collect_once().unwrap();
    assert_eq!(first.log_records, 3);
    assert!(
        spindle
            .cursors
            .values()
            .next()
            .unwrap()
            .btrfs_identity
            .is_none()
    );
    spindle
}

/// ADR-0028: an EOF poll must acquire stable identity before the next reboot.
#[test]
fn quiet_legacy_cursor_identity_migration_commits_without_replaying_rows() {
    let scratch = Scratch::new();
    let cfg = scratch.config();
    let mut spindle = legacy_quiet_spindle(&scratch, cfg.clone());
    let previous = spindle.cursors.values().next().unwrap().clone();
    let cycle = spindle
        .collect_with_log_reader(false, stable_quiet_read)
        .unwrap()
        .unwrap();
    assert_eq!((cycle.log_records, cycle.gaps), (0, 0));
    assert!(cycle.metric_points > 0);
    let migrated = spindle.cursors.values().next().unwrap().clone();
    assert_eq!(migrated.offset, previous.offset);
    assert_eq!(migrated.prefix_crc, previous.prefix_crc);
    assert!(migrated.btrfs_identity.is_some());
    drop(spindle);
    let reopened = Spindle::open_with_paths(cfg, scratch.paths()).unwrap();
    assert_eq!(reopened.cursors.values().next().unwrap(), &migrated);
}

#[test]
fn quiet_identity_migration_full_spool_preserves_legacy_cursor_and_retry() {
    let scratch = Scratch::new();
    let mut cfg = scratch.config();
    cfg.spool_bytes = 8192;
    let mut spindle = legacy_quiet_spindle(&scratch, cfg.clone());
    let previous = spindle.cursors.clone();
    let history = history_state(&spindle);
    loop {
        let filler = Batch {
            collection_gaps: vec!["capacity fixture".repeat(16)],
            ..Default::default()
        };
        if spindle.journal.append_owned(filler).is_err() {
            break;
        }
    }
    let sequence = spindle.journal.next_sequence();
    assert!(
        spindle
            .collect_with_log_reader(false, stable_quiet_read)
            .is_err()
    );
    assert_eq!(spindle.cursors, previous);
    assert_eq!(history_state(&spindle), history);
    assert_eq!(spindle.journal.next_sequence(), sequence);
    drop(spindle);
    cfg.spool_bytes = 1024 * 1024;
    let mut reopened = Spindle::open_with_paths(cfg, scratch.paths()).unwrap();
    assert_eq!(reopened.cursors, previous);
    let retry = reopened
        .collect_with_log_reader(false, stable_quiet_read)
        .unwrap()
        .unwrap();
    assert_eq!(retry.batch_sequence, sequence);
    assert_eq!(retry.log_records, 0);
    assert!(retry.gaps > 0); // Real capacity uncertainty, not a fabricated migration gap.
    assert!(
        reopened
            .cursors
            .values()
            .next()
            .unwrap()
            .btrfs_identity
            .is_some()
    );
}

#[test]
fn quiet_identity_migration_host_failure_uses_existing_failure_gap() {
    let scratch = Scratch::new();
    let mut spindle = legacy_quiet_spindle(&scratch, scratch.config());
    let history = history_state(&spindle);
    std::fs::remove_file(scratch.0.join("stat")).unwrap();
    let cycle = spindle
        .collect_with_log_reader(false, stable_quiet_read)
        .unwrap()
        .unwrap();
    assert_eq!(
        (cycle.log_records, cycle.metric_points, cycle.gaps),
        (0, 0, 1)
    );
    assert_eq!(history_state(&spindle), history);
    let mut batches = Vec::new();
    spindle
        .journal
        .replay(|batch| {
            batches.push(batch);
            Ok(())
        })
        .unwrap();
    assert!(batches[1].collection_gaps[0].starts_with("host metrics unavailable:"));
    assert!(batches[1].metrics.is_empty());
    assert!(batches[1].cursors[0].btrfs_identity.is_some());
}

#[test]
fn initial_empty_source_commits_identity_once_then_stays_quiet() {
    let scratch = Scratch::new();
    scratch.prepare();
    std::fs::write(scratch.0.join("input.log"), b"").unwrap();
    let mut spindle = Spindle::open_with_paths(scratch.config(), scratch.paths()).unwrap();
    assert!(spindle.cursors.is_empty());
    let first = spindle
        .collect_with_log_reader(false, stable_quiet_read)
        .unwrap()
        .unwrap();
    assert_eq!((first.log_records, first.gaps), (0, 0));
    assert!(first.metric_points > 0);
    let sequence = spindle.journal.next_sequence();
    let cursor = spindle.cursors.values().next().unwrap().clone();
    assert_eq!(cursor.offset, 0);
    assert!(cursor.btrfs_identity.is_some());
    std::fs::remove_file(scratch.0.join("stat")).unwrap();
    // Synthetic unchanged successful source observation, not another ext4
    // observation of the injected Btrfs namespace. No host sample may occur.
    let quiet = spindle
        .collect_with_log_reader(false, |_, prior, _, _| {
            Ok(log_source::ReadResult {
                cursor: prior.unwrap().clone(),
                lines: vec![],
                gaps: vec![],
                backlog_bytes: 0,
            })
        })
        .unwrap();
    assert!(quiet.is_none());
    assert_eq!(spindle.journal.next_sequence(), sequence);
    assert_eq!(spindle.cursors.values().next().unwrap(), &cursor);
}

#[test]
fn stable_skip_progress_commits_after_runtime_device_51_to_32() {
    let scratch = Scratch::new();
    scratch.prepare();
    let mut spindle = Spindle::open_with_paths(scratch.config(), scratch.paths()).unwrap();
    let first = spindle
        .collect_with_log_reader(false, |path, prior, budget, cost| {
            let mut read = stable_quiet_read(path, prior, budget, cost)?;
            read.cursor.device = 51;
            Ok(read)
        })
        .unwrap()
        .unwrap();
    assert_eq!((first.log_records, first.gaps), (0, 1));
    assert_eq!(spindle.cursors.values().next().unwrap().offset, MIB);
    let second = spindle
        .collect_with_log_reader(false, |_, prior, _, _| {
            // Successfully verified source outcome across the recorded device
            // renumbering. The adapter's actual checks have separate regressions.
            let mut cursor = prior.unwrap().clone();
            cursor.device = 32;
            cursor.offset = 2 * MIB;
            Ok(log_source::ReadResult {
                cursor,
                lines: vec![],
                gaps: vec![],
                backlog_bytes: MIB,
            })
        })
        .unwrap()
        .unwrap();
    assert_eq!((second.log_records, second.gaps), (0, 0));
    assert!(second.metric_points > 0);
    let cursor = spindle.cursors.values().next().unwrap();
    assert_eq!((cursor.device, cursor.offset), (32, 2 * MIB));
    assert!(cursor.skipping_oversize);
    assert!(cursor.btrfs_identity.is_some());
}
