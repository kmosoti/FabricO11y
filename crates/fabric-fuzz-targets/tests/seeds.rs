//! Writes the seed inputs of fuzz/corpus. Run on demand, not in CI:
//! `cargo test -p fabric-fuzz-targets --test seeds -- --ignored`.

use fabric_frame::envelope::{Batch, Cursor};
use fabric_frame::frame::{ACTIVE, FrameLog};
use prost::Message;
use std::path::{Path, PathBuf};

fn corpus(target: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz/corpus")
        .join(target);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn batch(sequence: u64, gaps: Vec<String>, logs: Vec<u8>) -> Vec<u8> {
    Batch {
        version: 1,
        node_id: vec![7; 16],
        generation: 1,
        sequence,
        metrics: Vec::new(),
        logs,
        cursors: vec![Cursor {
            path: "/var/log/app.log".into(),
            device: 1,
            inode: 2,
            offset: 128,
            skipping_oversize: false,
            prefix_len: 64,
            prefix_crc: 99,
        }],
        collection_gaps: gaps,
        traces: Vec::new(),
    }
    .encode_to_vec()
}

#[test]
#[ignore]
fn write_seeds() {
    let dir = corpus("batch_identify");
    std::fs::write(
        dir.join("valid-gap-batch"),
        batch(1, vec!["log source unavailable /x".into()], Vec::new()),
    )
    .unwrap();
    std::fs::write(
        dir.join("valid-log-batch"),
        batch(42, Vec::new(), vec![10, 3, 1, 2, 3]),
    )
    .unwrap();

    let dir = corpus("query_request");
    for (name, body) in [
        (
            "logs",
            r#"{"kind":"logs","node":"n1","from_ns":0,"to_ns":100,"contains":"err","limit":10}"#,
        ),
        (
            "metrics",
            r#"{"kind":"metrics","name":"system.cpu.time","from_ns":5,"to_ns":9,"limit":1}"#,
        ),
        (
            "rate",
            r#"{"kind":"rate","name":"c","from_ns":0,"to_ns":18446744073709551615}"#,
        ),
        (
            "page",
            r#"{"kind":"logs","from_ns":0,"to_ns":10,"limit":2,"page":"7b226b223a317d"}"#,
        ),
    ] {
        std::fs::write(dir.join(name), body).unwrap();
    }

    let dir = corpus("frame_recovery");
    let scratch = std::env::temp_dir().join(format!("fabric-seed-frames-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    {
        let mut log = FrameLog::open(&scratch, 1 << 30, 1 << 20, |_, _| Ok(())).unwrap();
        for payload in [&b"first"[..], b"second frame", &[0_u8; 300]] {
            log.append(payload).unwrap();
        }
    }
    let bytes = std::fs::read(scratch.join(ACTIVE)).unwrap();
    std::fs::write(dir.join("three-frames"), &bytes).unwrap();
    std::fs::write(dir.join("torn-tail"), &bytes[..bytes.len() - 7]).unwrap();
    std::fs::write(dir.join("empty"), b"").unwrap();
    std::fs::remove_dir_all(&scratch).unwrap();
}
