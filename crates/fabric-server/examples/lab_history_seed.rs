//! Private lab fixture: exact deterministic old-source ledger through production custody.
//! Run only under the coordinator resource envelope, with a stopped fresh server.
use fabric_frame::{
    envelope::Batch,
    frame::{FileRef, FrameLog},
};
use fabric_server::{
    config::Config,
    control::{Control, DesiredConfig},
    segment,
    store::{CommitMode, Group, Store, Submission, identify_strand},
};
use opentelemetry_proto::tonic::{
    collector::logs::v1::ExportLogsServiceRequest,
    common::v1::{AnyValue, KeyValue, any_value},
    logs::v1::{LogRecord, ResourceLogs, ScopeLogs},
};
use prost::Message;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Write},
};

const OLD_NS: u64 = 1_759_680_000_000_000_000; // frozen 2025-10-05; retention uses production receive times
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn body(seq: u64, index: usize, len: usize) -> String {
    let mut s = format!("old-{seq:06}:{index:03} ");
    if index.is_multiple_of(2) {
        s.extend(std::iter::repeat_n('R', len.saturating_sub(s.len())));
    } else {
        let mut counter = 0;
        while s.len() < len {
            s.push_str(&hex(&Sha256::digest(format!(
                "2703163393:{seq}:{index}:{counter}"
            ))));
            counter += 1;
        }
    }
    s.truncate(len);
    s
}
fn encoded(seq: u64, bodies: &[String], padding: usize) -> Vec<u8> {
    let logs = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: bodies
                    .iter()
                    .enumerate()
                    .map(|(i, s)| LogRecord {
                        observed_time_unix_nano: OLD_NS + seq * 1_000_000 + i as u64,
                        body: Some(AnyValue {
                            value: Some(any_value::Value::StringValue(s.clone())),
                        }),
                        attributes: if i == 0 && padding > 0 {
                            vec![KeyValue {
                                key: "fixture.padding".into(),
                                value: Some(AnyValue {
                                    value: Some(any_value::Value::StringValue("P".repeat(padding))),
                                }),
                                ..Default::default()
                            }]
                        } else {
                            vec![]
                        },
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    Batch {
        version: 1,
        node_id: vec![0x71; 16],
        generation: 1,
        sequence: seq,
        logs: logs.encode_to_vec(),
        metrics: vec![],
        traces: vec![],
        cursors: vec![],
        collection_gaps: vec![],
    }
    .encode_to_vec()
}
fn run() -> io::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() != 3 && a.len() != 4 {
        return Err(io::Error::other(
            "usage: lab_history_seed CONFIG H0|H256|nearrotation LEDGER [TARGET_BYTES]",
        ));
    }
    let cfg = Config::load(&a[0])?;
    let history = a[1] == "H256";
    if !history && a[1] != "nearrotation" && a[1] != "H0" {
        return Err(io::Error::other("unknown seed condition"));
    }
    let (_, token) = Control::open(&cfg.state_dir)?.enroll(
        "oldhistory",
        DesiredConfig {
            logs: vec![],
            metric_interval_s: 15,
        },
    )?;
    // Preserve enrollment credential and binding, never rewrite native Spool state.
    std::fs::write(cfg.state_dir.join("oldhistory.token"), format!("{token}\n"))?;
    let default_target = if history {
        256 * 1024 * 1024
    } else if a[1] == "nearrotation" {
        64 * 1024 * 1024 - 128 * 1024
    } else {
        0
    };
    let target = if a.len() == 4 {
        a[3].parse::<usize>().map_err(io::Error::other)?
    } else {
        default_target
    };
    let mut store = Store::open_with(
        &cfg.state_dir,
        cfg.journal_bytes,
        cfg.journal_file_bytes,
        CommitMode::INDIVIDUAL,
    )?;
    let mut ledger = std::io::BufWriter::new(File::create(&a[2])?);
    let mut expected = std::collections::BTreeMap::new();
    let mut total = 0;
    let mut seq = 1;
    let mut rows = 0;
    let mut prefix = Sha256::new();
    while total < target {
        let mut bodies: Vec<_> = (0..512).map(|i| body(seq, i, 900)).collect();
        let mut bytes = encoded(seq, &bodies, 0);
        if bytes.len() > target - total {
            let remaining = target - total;
            let mut found = None;
            // Bodies remain 900 bytes. Only the final Batch has declared padding
            // attributes, allowing an exact encoded-size target without giant rows.
            'search: for count in (1..=512).rev() {
                let candidate: Vec<_> = (0..count).map(|i| body(seq, i, 900)).collect();
                let base = encoded(seq, &candidate, 0);
                if base.len() > remaining {
                    continue;
                }
                if base.len() == remaining {
                    found = Some((candidate, base));
                    break;
                }
                let mut lo = 1;
                let mut hi = remaining - base.len() + 1;
                while lo <= hi {
                    let mid = lo + (hi - lo) / 2;
                    let raw = encoded(seq, &candidate, mid);
                    if raw.len() == remaining {
                        found = Some((candidate, raw));
                        break 'search;
                    }
                    if raw.len() < remaining {
                        lo = mid + 1;
                    } else {
                        hi = mid - 1;
                    }
                }
            }
            let Some((b, r)) = found else {
                return Err(io::Error::other("cannot fit exact encoded prefix"));
            };
            bodies = b;
            bytes = r;
        }
        let (strand, sequence) = identify_strand(&bytes)?;
        let (reply, rx) = tokio::sync::oneshot::channel();
        let hash = hex(&Sha256::digest(&bytes));
        prefix.update(&bytes);
        store.commit(vec![Submission {
            label: "oldhistory".into(),
            strand,
            sequence,
            bytes: bytes.clone(),
            reply,
        }]);
        let answer = rx.blocking_recv().map_err(io::Error::other)?;
        if answer != fabric_app::delivery::Answer::Ack(sequence.get()) {
            return Err(io::Error::other(format!(
                "seed custody refused: {answer:?}"
            )));
        }
        let source: Vec<_> = bodies
            .iter()
            .map(|b| {
                serde_json::json!([
                    b.split(' ').next().unwrap(),
                    hex(&Sha256::digest(b.as_bytes()))
                ])
            })
            .collect();
        writeln!(
            ledger,
            "{}",
            serde_json::json!({"label":"oldhistory","sequence":seq,"sha256":&hash,"bytes":bytes.len(),"source":source})
        )?;
        expected.insert(seq, (hash, bytes.len()));
        total += bytes.len();
        rows += bodies.len();
        seq += 1;
    }
    ledger.flush()?;
    drop(store);
    if history {
        let journal = cfg.state_dir.join("journal");
        let mut first = None;
        let mut frames = FrameLog::open(
            &journal,
            cfg.journal_bytes,
            segment::MAX_GROUP_PAYLOAD,
            |payload, pos| {
                if matches!(pos.file, FileRef::Active) && first.is_none() {
                    first = Some(
                        Group::decode(payload)
                            .map_err(io::Error::other)?
                            .group_sequence,
                    );
                }
                Ok(())
            },
        )?;
        frames.rotate(first.ok_or_else(|| io::Error::other("missing active prefix"))?)?;
        drop(frames);
        let mut files: Vec<_> = std::fs::read_dir(&journal)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("sealed-")
            })
            .collect();
        files.sort();
        for path in files {
            let label = path
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .trim_start_matches("sealed-")
                .parse()
                .map_err(io::Error::other)?;
            segment::build_sealed(&cfg.state_dir, label, &path)?;
        }
        drop(Store::open_with(
            &cfg.state_dir,
            cfg.journal_bytes,
            cfg.journal_file_bytes,
            CommitMode::INDIVIDUAL,
        )?);
    }
    let mut recovered = std::collections::BTreeMap::new();
    let mut receive_min = u64::MAX;
    let mut receive_max = 0;
    Store::replay(&cfg.state_dir, cfg.journal_bytes, |entry| {
        receive_min = receive_min.min(entry.received_unix_nano);
        receive_max = receive_max.max(entry.received_unix_nano);
        let batch = Batch::decode(entry.batch.as_slice()).map_err(io::Error::other)?;
        if entry.label != "oldhistory" || batch.node_id != vec![0x71; 16] || batch.generation != 1 {
            return Err(io::Error::other("seed replay identity changed"));
        }
        if recovered
            .insert(
                batch.sequence,
                (hex(&Sha256::digest(&entry.batch)), entry.batch.len()),
            )
            .is_some()
        {
            return Err(io::Error::other("duplicate seed replay"));
        }
        Ok(())
    })?;
    if recovered != expected {
        return Err(io::Error::other(
            "seed byte custody differs after publication",
        ));
    }
    println!(
        "{}",
        serde_json::json!({"received_min_ns":if total>0 {Some(receive_min)} else {None},"received_max_ns":if total>0 {Some(receive_max)} else {None},"fixture_target_override":a.len()==4,"seed_replay_exact":true,"old_node_id":"71717171717171717171717171717171","generation":1,"condition":a[1],"encoded_bytes":total,"batches":seq-1,"rows":rows,"prefix_sha256":hex(&prefix.finalize()),"observed_start_ns":OLD_NS,"segments":segment::list(&cfg.state_dir)?})
    );
    Ok(())
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lab_history_seed: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
