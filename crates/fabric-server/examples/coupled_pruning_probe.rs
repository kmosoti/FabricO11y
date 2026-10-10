//! Additive O7 soundness fixture: actual group identities, bounds and full chains.
use arrow_array::{FixedSizeBinaryArray, Int64Array, StringArray, UInt32Array, UInt64Array};
use fabric_frame::{envelope::Batch, frame::FrameLog};
use fabric_server::{
    query::{History, Plan, Query},
    segment,
    store::{CommitMode, Entry, Group, Store},
};
use opentelemetry_proto::tonic::{
    collector::logs::v1::ExportLogsServiceRequest,
    common::v1::{AnyValue, any_value},
    logs::v1::{LogRecord, ResourceLogs, ScopeLogs},
};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use prost::Message;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::PathBuf,
};
const START: u64 = 1_800_000_000_000_000_000;
const ROWS: usize = 1100;
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn main() {
    let root = PathBuf::from(std::env::args().nth(1).expect("owned scratch directory"));
    assert!(!root.exists());
    fs::create_dir(&root).unwrap();
    let state = root.join("state");
    drop(Store::open(&state, 1024 * 1024 * 1024, CommitMode::INDIVIDUAL).unwrap());
    let input = root.join("input");
    fs::create_dir(&input).unwrap();
    let mut journal = FrameLog::open(
        &input,
        64 * 1024 * 1024,
        segment::MAX_GROUP_PAYLOAD,
        |_, _| Ok(()),
    )
    .unwrap();
    let mut ledger = BufWriter::new(File::create(root.join("records.jsonl")).unwrap());
    for chunk in 0..ROWS.div_ceil(32) {
        let first = chunk * 32;
        let logs = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                scope_logs: vec![ScopeLogs {
                    log_records: (first..(first + 32).min(ROWS))
                        .map(|i| {
                            let prefix = format!("O7-{i:04} ");
                            let body = prefix.clone() + &"x".repeat(16 * 1024 - prefix.len());
                            // An older final arrival and a common timestamp force ties across every group boundary.
                            LogRecord {
                                observed_time_unix_nano: START
                                    + if i == 0 {
                                        10
                                    } else if i == ROWS - 1 {
                                        1
                                    } else {
                                        5
                                    },
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
        }
        .encode_to_vec();
        let batch = Batch {
            version: 1,
            node_id: vec![7; 16],
            generation: 1,
            sequence: chunk as u64 + 1,
            logs,
            ..Default::default()
        };
        batch.validate().unwrap();
        let raw = batch.encode_to_vec();
        let entry = Entry {
            label: "fixture".into(),
            received_unix_nano: START + chunk as u64 + 1,
            batch: raw.clone(),
        };
        writeln!(ledger,"{}",json!({"label":entry.label,"received_ns":entry.received_unix_nano,"hex":hex(&raw),"sha256":hex(&Sha256::digest(&raw))})).unwrap();
        journal
            .append(
                &Group {
                    group_sequence: chunk as u64 + 1,
                    entries: vec![entry],
                }
                .encode_to_vec(),
            )
            .unwrap();
    }
    ledger.flush().unwrap();
    drop(ledger);
    drop(journal);
    let manifest =
        segment::build_sealed(&state, 1, &input.join(fabric_frame::frame::ACTIVE)).unwrap();
    let dir = segment::segments_dir(&state)
        .unwrap()
        .join(segment::segment_name(1));
    segment::verify(&dir, &manifest).unwrap();
    let bounds = segment::row_group_bounds(&dir, &manifest, segment::Table::Logs).unwrap();
    assert!(
        bounds.len() >= 2,
        "must force actual byte-capped row groups"
    );
    let mut groups = Vec::new();
    for &(id, min, max) in &bounds {
        let reader =
            ParquetRecordBatchReaderBuilder::try_new(File::open(dir.join("logs.parquet")).unwrap())
                .unwrap()
                .with_row_groups(vec![id])
                .build()
                .unwrap();
        let mut rows = Vec::new();
        for batch in reader {
            let b = batch.unwrap();
            let node = b
                .column(2)
                .as_any()
                .downcast_ref::<FixedSizeBinaryArray>()
                .unwrap();
            let seq = b.column(3).as_any().downcast_ref::<UInt64Array>().unwrap();
            let index = b.column(4).as_any().downcast_ref::<UInt32Array>().unwrap();
            let times = b.column(5).as_any().downcast_ref::<Int64Array>().unwrap();
            let body = b.column(6).as_any().downcast_ref::<StringArray>().unwrap();
            for i in 0..b.num_rows() {
                rows.push(json!({"node_id":hex(node.value(i)),"sequence":seq.value(i),"index":index.value(i),"time":times.value(i) as u64,"body_sha256":hex(&Sha256::digest(body.value(i).as_bytes()))}));
            }
        }
        groups.push(json!({"id":id,"min":min,"max":max,"rows":rows}));
    }
    let mut windows = vec![
        (START + 2, START + 3),
        (START - 1, START),
        (START, START + 11),
        (START + 11, START + 12),
    ];
    for &(_, min, max) in &bounds {
        windows.push((min, min + 1));
        windows.push((max, max + 1));
    }
    windows.sort_unstable();
    windows.dedup();
    let mut reports = Vec::new();
    for (w, &(from, to)) in windows.iter().enumerate() {
        let query = json!({"kind":"logs","from_ns":from,"to_ns":to,"limit":50});
        let admitted: Vec<_> = bounds
            .iter()
            .filter(|(_, min, max)| *max >= from && *min < to)
            .map(|(id, _, _)| *id)
            .collect();
        for (name, plan) in [("walk", Plan::Walk), ("scan", Plan::Scan)] {
            let history = History::with_plan(&state, plan);
            let mut request = query.clone();
            let mut chain =
                BufWriter::new(File::create(root.join(format!("chain-{w}-{name}.jsonl"))).unwrap());
            let mut pages = 0;
            loop {
                let parsed: Query = serde_json::from_value(request.clone()).unwrap();
                let page: Value = history.run(&parsed, ROWS.div_ceil(32) as u64).unwrap();
                assert_eq!(page["complete"], true);
                writeln!(chain, "{page}").unwrap();
                pages += 1;
                assert!(pages <= 24);
                if page["next_page"].is_null() {
                    break;
                }
                request["page"] = page["next_page"].clone();
            }
            chain.flush().unwrap();
        }
        reports.push(json!({"id":w,"query":query,"admitted":admitted}));
    }
    println!(
        "{}",
        json!({"fixture":{"rows":ROWS,"body_bytes":16384,"shared_timestamp":true,"late_final_arrival":true,"run_mib_selector":option_env!("FABRIC_RUN_MIB_EXPERIMENT").unwrap_or("16")},"groups":groups,"windows":reports,"manifest":manifest})
    );
}
