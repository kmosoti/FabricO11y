//! Deterministic journal-identity regressions following the pressure HTTP500s.
//! Four cases use real FrameLog IO and producer-defined bytes/rows, without
//! treating Scan, Walk, extract(), or a changed oracle as the expected answer.

use super::{TailEntry, TailReader, WalkState};
use crate::segment::MAX_GROUP_PAYLOAD;
use crate::store::{Entry, Group};
use fabric_frame::envelope::Batch;
use fabric_frame::frame::{ACTIVE, FrameLog};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use prost::Message;
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new(case: &str) -> Self {
        let base = PathBuf::from(
            std::env::var("FABRIC_SCRATCH_ROOT")
                .expect("run through the resource launcher; no scratch fallback"),
        );
        let path = base.join(format!(
            "hammer-tail-identity-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::write(path.join("origin.json"), serde_json::json!({
            "case": case,
            "seed": "deterministic-no-randomness",
            "origin": "source-inspection candidate following pressure-walk-01 HTTP500; causal link unproved",
            "contract": "HIST-1/HIST-2/HIST-7: journal movement preserves exact snapshot rows; genuine corruption remains an error",
            "outcome": "recorded by the enclosing test command, not inferred here"
        }).to_string()).unwrap();
        Self(path)
    }

    fn journal(&self) -> FrameLog {
        FrameLog::open(&self.0, 1 << 20, MAX_GROUP_PAYLOAD, |_, _| Ok(())).unwrap()
    }

    fn producer(&self, groups: &[Group]) {
        let expected: Vec<_> = groups
            .iter()
            .map(|g| {
                serde_json::json!({
                    "group": g.group_sequence,
                    "label": g.entries[0].label,
                    "received_ns": g.entries[0].received_unix_nano,
                    "exact_batch_bytes": g.entries[0].batch
                })
            })
            .collect();
        fs::write(
            self.0.join("producer-before-query.json"),
            serde_json::to_vec_pretty(&expected).unwrap(),
        )
        .unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "tail identity failure fixture retained: {}",
                self.0.display()
            );
        } else {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}

fn group(sequence: u64, body: &str) -> Group {
    let logs = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: vec![LogRecord {
                    observed_time_unix_nano: 1_000 + sequence,
                    body: Some(AnyValue {
                        value: Some(any_value::Value::StringValue(body.into())),
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    let batch = Batch {
        version: 1,
        node_id: vec![7; 16],
        generation: 1,
        sequence,
        metrics: vec![],
        logs: logs.encode_to_vec(),
        traces: vec![],
        cursors: vec![],
        collection_gaps: vec![],
    };
    batch.validate().unwrap();
    Group {
        group_sequence: sequence,
        entries: vec![Entry {
            label: "original-node".into(),
            batch: batch.encode_to_vec(),
            received_unix_nano: 2_000 + sequence,
        }],
    }
}

fn indexed(
    state: &mut WalkState,
    scratch: &Scratch,
    wanted: u64,
) -> (std::collections::HashMap<u64, PathBuf>, TailEntry) {
    let paths = state.extend(&scratch.0, &[]).unwrap();
    assert!(
        state.blocks.is_empty(),
        "fixture must exercise actual lazy journal reads"
    );
    let entry = *state.entries.iter().find(|e| e.group == wanted).unwrap();
    (paths, entry)
}

fn exact_old(reader: &mut TailReader<'_>, entry: &TailEntry, expected: &Group, body: &str) {
    // Assert rows first: this is the production operation under investigation.
    let rows = reader
        .rows(entry)
        .expect("sealed original is still available: exact old rows required");
    assert_eq!(rows.logs.len(), 1);
    let row = &rows.logs[0];
    assert_eq!(row.group, expected.group_sequence);
    assert_eq!(row.node, "original-node");
    assert_eq!(row.node_id, [7; 16]);
    assert_eq!(row.sequence, expected.group_sequence);
    assert_eq!(row.index, 0);
    assert_eq!(row.observed_ns, 1_000 + expected.group_sequence);
    assert_eq!(row.body, body);
    assert!(row.attributes.is_empty());
    // Also check original custody bytes, independently of row extraction.
    let actual = reader.group(entry.file_first, entry.offset).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn a_replaced_active_path_does_not_substitute_a_new_group_at_the_same_offset() {
    let scratch = Scratch::new("late-open-same-offset-substitution");
    let old = group(1, "OLD-original-body");
    let new = group(2, "NEW-replacement-body");
    scratch.producer(&[old.clone(), new.clone()]);
    let mut log = scratch.journal();
    log.append(&old.encode_to_vec()).unwrap();
    let mut state = WalkState::default();
    let (paths, entry) = indexed(&mut state, &scratch, 1);
    assert_eq!(entry.offset, 0);
    log.rotate(1).unwrap();
    log.append(&new.encode_to_vec()).unwrap();
    assert!(scratch.0.join("sealed-00000000000000000001.faj").exists());
    exact_old(
        &mut TailReader::new(&paths),
        &entry,
        &old,
        "OLD-original-body",
    );
}

#[test]
fn a_late_old_offset_beyond_the_replacement_file_reads_the_sealed_original() {
    let scratch = Scratch::new("late-open-offset-beyond-replacement");
    let padding = group(1, &"padding".repeat(1_000));
    let old = group(2, "OLD-late-offset");
    let new = group(3, "NEW-short");
    scratch.producer(&[padding.clone(), old.clone(), new.clone()]);
    let mut log = scratch.journal();
    log.append(&padding.encode_to_vec()).unwrap();
    log.append(&old.encode_to_vec()).unwrap();
    let mut state = WalkState::default();
    let (paths, entry) = indexed(&mut state, &scratch, 2);
    log.rotate(1).unwrap();
    log.append(&new.encode_to_vec()).unwrap();
    assert!(entry.offset > fs::metadata(scratch.0.join(ACTIVE)).unwrap().len());
    exact_old(
        &mut TailReader::new(&paths),
        &entry,
        &old,
        "OLD-late-offset",
    );
}

#[test]
fn genuine_corruption_in_the_same_file_is_not_reclassified_as_movement() {
    let scratch = Scratch::new("same-file-real-checksum-corruption");
    let old = group(1, "OLD-corruption-control");
    scratch.producer(std::slice::from_ref(&old));
    let mut log = scratch.journal();
    log.append(&old.encode_to_vec()).unwrap();
    let mut state = WalkState::default();
    let (paths, entry) = indexed(&mut state, &scratch, 1);
    drop(log);
    // Corrupt one payload byte while preserving a valid header and length.
    // The data frame's payload starts at byte 16 (fabric-frame/frame.rs).
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(scratch.0.join(ACTIVE))
        .unwrap();
    file.seek(SeekFrom::Start(16)).unwrap();
    let mut byte = [0];
    file.read_exact(&mut byte).unwrap();
    file.seek(SeekFrom::Start(16)).unwrap();
    file.write_all(&[byte[0] ^ 1]).unwrap();
    file.sync_all().unwrap();
    let error = TailReader::new(&paths)
        .rows(&entry)
        .err()
        .expect("genuine corruption must remain an error");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(
        error
            .to_string()
            .contains("frame payload checksum mismatch"),
        "{error}"
    );
}

#[test]
fn rotation_between_discovery_and_indexing_never_admits_a_split_identity() {
    let scratch = Scratch::new("discovery-to-index-reopen-cut");
    let old = group(1, "OLD-before-discovery");
    let new = group(2, "NEW-after-rotation");
    scratch.producer(&[old.clone(), new.clone()]);
    let mut log = scratch.journal();
    log.append(&old.encode_to_vec()).unwrap();
    let mut state = WalkState::default();
    // The accompanying .patch adds only this deterministic private callback.
    let result = state.extend_at_discovery_cut(&scratch.0, &[], || {
        log.rotate(1).unwrap();
        log.append(&new.encode_to_vec()).unwrap();
    });
    // A coherent old-handle result or explicit movement is allowed at this cut.
    // A non-movement IO error, or identity substitution, fails unchanged.
    match result {
        Ok(paths) => {
            assert_eq!(state.entries.len(), 1);
            let entry = *state
                .entries
                .iter()
                .find(|e| e.group == 1)
                .expect("captured old file must not become a future group");
            assert_eq!(entry.file_first, 1);
            assert_eq!(entry.offset, 0);
            exact_old(
                &mut TailReader::new(&paths),
                &entry,
                &old,
                "OLD-before-discovery",
            );
        }
        Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::Interrupted, "{error}"),
    }
    // Fresh acquisition must contain both producer groups exactly once even
    // if an earlier interrupted attempt partially mutated the derived index.
    let paths = state.extend(&scratch.0, &[]).unwrap();
    let groups: Vec<_> = state.entries.iter().map(|e| e.group).collect();
    assert_eq!(groups, vec![1, 2]);
    for (expected, body) in [(&old, "OLD-before-discovery"), (&new, "NEW-after-rotation")] {
        let entry = state
            .entries
            .iter()
            .find(|e| e.group == expected.group_sequence)
            .unwrap();
        exact_old(&mut TailReader::new(&paths), entry, expected, body);
    }
}
