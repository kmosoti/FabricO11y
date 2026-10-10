//! Native counterexample: admission must never hide reader validation.
use arrow_array::{RecordBatch, StringArray};
use fabric_frame::envelope::Batch;
use fabric_server::{
    query::{History, Plan, Query},
    segment,
    store::{CommitMode, Entry, Group, Store},
};
use opentelemetry_proto::tonic::{
    collector::logs::v1::ExportLogsServiceRequest,
    logs::v1::{LogRecord, ResourceLogs, ScopeLogs},
};
use parquet::arrow::{ArrowWriter, arrow_reader::ParquetRecordBatchReaderBuilder};
use prost::Message;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, sync::Arc};

#[test]
fn key_rejected_corrupt_attributes_still_make_history_incomplete() {
    let root = PathBuf::from(std::env::var("FABRIC_SCRATCH_ROOT").expect("contained scratch"))
        .join(format!("key-first-corruption-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    fs::write(root.join("origin.json"), json!({"seed":42,"defect":"last full-key rejected row has invalid attributes JSON","contract":"reader validates before admission","key_first":History::key_first_enabled()}).to_string()).unwrap();
    drop(Store::open(&root, 1 << 30, CommitMode::INDIVIDUAL).unwrap());
    let request = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: (0..4)
                    .map(|i| LogRecord {
                        observed_time_unix_nano: 100 + i,
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    let batch = Batch {
        version: 1,
        node_id: vec![7; 16],
        generation: 1,
        sequence: 1,
        logs: request.encode_to_vec(),
        ..Default::default()
    };
    segment::build(
        &root,
        1,
        &[Group {
            group_sequence: 1,
            entries: vec![Entry {
                label: "control".into(),
                batch: batch.encode_to_vec(),
                received_unix_nano: 200,
            }],
        }],
    )
    .unwrap();
    let dir = root.join("segments").join(segment::segment_name(1));
    let path = dir.join("logs.parquet");
    let query: Query =
        serde_json::from_value(json!({"kind":"logs","from_ns":0,"to_ns":1000,"limit":1})).unwrap();
    for plan in [Plan::Scan, Plan::Walk] {
        assert_eq!(
            History::with_plan(&root, plan).run(&query, 1).unwrap()["complete"],
            true
        );
    }
    let valid = ParquetRecordBatchReaderBuilder::try_new(fs::File::open(&path).unwrap())
        .unwrap()
        .build()
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(valid.num_rows(), 4);
    // First two keys fill limit+1; the last key cannot enter the canonical heap.
    let mut arrays = valid.columns().to_vec();
    arrays[7] = Arc::new(StringArray::from(vec!["{}", "{}", "{}", "not-json"]));
    let corrupt = RecordBatch::try_new(valid.schema(), arrays).unwrap();
    let mut writer =
        ArrowWriter::try_new(fs::File::create(&path).unwrap(), corrupt.schema(), None).unwrap();
    writer.write(&corrupt).unwrap();
    writer.close().unwrap();
    // Preserve the valid envelope: failure must come from typed JSON validation,
    // rather than an earlier file-size/schema/digest mismatch.
    let mut manifest = segment::read_manifest(&dir).unwrap();
    let raw = fs::read(&path).unwrap();
    let entry = manifest.files.get_mut("logs.parquet").unwrap();
    entry.bytes = raw.len() as u64;
    entry.sha256 = format!("{:x}", Sha256::digest(&raw));
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let mut answers = Vec::new();
    for plan in [Plan::Scan, Plan::Walk] {
        let answer = History::with_plan(&root, plan).run(&query, 1).unwrap();
        assert_eq!(
            answer["complete"], false,
            "reader error hidden by admission"
        );
        assert_eq!(answer["unavailable"].as_array().unwrap().len(), 1);
        answers.push(answer);
    }
    fs::write(
        root.join("outcomes.json"),
        serde_json::to_vec(&answers).unwrap(),
    )
    .unwrap();
    fn members(
        root: &std::path::Path,
        dir: &std::path::Path,
        map: &mut serde_json::Map<String, serde_json::Value>,
    ) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            assert!(!path.is_symlink(), "control member cannot be a link");
            if path.is_dir() {
                members(root, &path, map);
            } else {
                assert!(path.is_file(), "control member must be regular");
                let raw = fs::read(&path).unwrap();
                map.insert(
                    path.strip_prefix(root).unwrap().to_string_lossy().into(),
                    json!({"bytes":raw.len(),"sha256":format!("{:x}",Sha256::digest(raw))}),
                );
            }
        }
    }
    let mut map = serde_json::Map::new();
    members(&root, &root, &mut map);
    println!(
        "{}",
        json!({"origin":serde_json::from_slice::<serde_json::Value>(&fs::read(root.join("origin.json")).unwrap()).unwrap(),"outcomes":answers,"members":map})
    );
    // Only a successful assertion path removes the owned control; panics retain it.
    fs::remove_dir_all(&root).unwrap();
}
