//! Command oracle written before adapt-otlp implementation; caller-authored input.
use fabric_o11y::{EventId, Payload, log::EventLog};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

fn run(root: &Path, args: &[&str], exit: i32) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_fabric-research"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(exit),
        "args={args:?} stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    if exit == 0 {
        serde_json::from_slice(&out.stdout).unwrap()
    } else {
        assert!(!out.stderr.is_empty());
        Value::Null
    }
}

#[test]
fn offline_logs_reach_verified_complete_answer_across_processes() {
    let root = std::env::temp_dir().join(format!("prototype-collect-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let request = json!({"resourceLogs":[{"scopeLogs":[{"logRecords":[
        {"timeUnixNano":"10","body":{"stringValue":"rare first"}},
        {"timeUnixNano":"5","body":{"stringValue":"ordinary"}},
        {"timeUnixNano":"10","body":{"stringValue":"rare\u{2003}last"}}
    ]}]}]});
    let request_bytes = serde_json::to_vec(&request).unwrap();
    let config = br#"{"tenant":7,"source":8,"resource":9,"first_event_id":100}"#;
    fs::write(root.join("request.json"), &request_bytes).unwrap();
    fs::write(root.join("config.json"), config).unwrap();
    let report = run(
        &root,
        &["adapt-otlp", "request.json", "config.json", "input.json"],
        0,
    );
    assert_eq!(report["adapted_events"], 3);
    assert_eq!(report["output"], "input.json");
    let input = fs::read(root.join("input.json")).unwrap();
    let rows = storage_probe::disk::decode_events(&input).unwrap();
    assert_eq!(rows.len(), 3);
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row.id, EventId(100 + i as u64));
        assert_eq!(row.tenant.0, 7);
        assert_eq!(row.source.0, 8);
        assert_eq!(row.resource.0, 9);
        assert_eq!(row.event_time.0, [10, 5, 10][i]);
        assert_eq!(row.observed_time.0, 0);
        match &row.payload {
            Payload::Log { body } => {
                assert_eq!(body, ["rare first", "ordinary", "rare\u{2003}last"][i])
            }
            _ => panic!("wrong payload"),
        }
    }
    run(
        &root,
        &["adapt-otlp", "request.json", "config.json", "input.json"],
        1,
    );
    assert_eq!(fs::read(root.join("input.json")).unwrap(), input);
    let ingest = run(&root, &["ingest", "input.json", "events.fol", "2", "1"], 0);
    assert_eq!(ingest["appended"], 3);
    let mut log = EventLog::open(root.join("events.fol")).unwrap();
    let mut ids = Vec::new();
    log.replay(|row| {
        ids.push(row.id.0);
        Ok(())
    })
    .unwrap();
    assert_eq!(ids, [100, 101, 102]);
    drop(log);
    run(
        &root,
        &[
            "publish",
            "events.fol",
            "snapshot",
            "trusted.json",
            "1",
            "42",
        ],
        0,
    );
    fs::write(
        root.join("query.json"),
        br#"{"start_ns":0,"end_ns":20,"tenant":7,"token":"rare"}"#,
    )
    .unwrap();
    let first = run(
        &root,
        &[
            "query",
            "snapshot",
            "trusted.json",
            "query.json",
            "none",
            "first.json",
        ],
        0,
    );
    assert_eq!(first["complete"], false);
    assert_eq!(first["positions"], json!([]));
    let second = run(
        &root,
        &[
            "resume",
            "snapshot",
            "trusted.json",
            "query.json",
            "first.json",
            first["checkpoint_sha256"].as_str().unwrap(),
            "all",
            "second.json",
        ],
        0,
    );
    assert_eq!(second["complete"], true);
    assert_eq!(second["positions"], json!([0, 2]));
    let verified = run(
        &root,
        &[
            "verify",
            "input.json",
            "trusted.json",
            "query.json",
            "second.json",
            second["checkpoint_sha256"].as_str().unwrap(),
        ],
        0,
    );
    assert_eq!(verified["verified"], true);
    assert_eq!(verified["matched"], 2);
    for malformed in [
        r#"{"tenant":7,"tenant":8,"source":8,"resource":9,"first_event_id":100}"#,
        r#"{"tenant":"7","source":8,"resource":9,"first_event_id":100}"#,
        r#"{"tenant":7,"source":8,"resource":9}"#,
        r#"{"tenant":7,"source":8,"resource":9,"first_event_id":100,"extra":0}"#,
    ] {
        fs::write(root.join("bad-config.json"), malformed).unwrap();
        run(
            &root,
            &[
                "adapt-otlp",
                "request.json",
                "bad-config.json",
                "rejected.json",
            ],
            1,
        );
        assert!(!root.join("rejected.json").exists());
    }
    fs::write(root.join("bad-request.json"), br#"{"resourceLogs":[{"scopeLogs":[{"logRecords":[{"body":{"stringValue":"ok"}},{"body":{"bytesValue":"bad"}}]}]}]}"#).unwrap();
    run(
        &root,
        &[
            "adapt-otlp",
            "bad-request.json",
            "config.json",
            "rejected.json",
        ],
        1,
    );
    assert!(!root.join("rejected.json").exists());
    assert_eq!(fs::read(root.join("request.json")).unwrap(), request_bytes);
    assert_eq!(fs::read(root.join("config.json")).unwrap(), config);
    fs::remove_dir_all(&root).unwrap();
}
