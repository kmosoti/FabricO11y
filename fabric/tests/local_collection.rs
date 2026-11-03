use fabric::{
    observation::{self, Data, Identity, Level, MetricType, Observation},
    sdk,
    spindle::{
        self,
        custody::{Checkpoint, Store},
        sources::{FileConfig, FileFormat, FileSource},
    },
};
use std::{
    fs,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
// Contended is a permitted immediate outcome while the delivery worker owns the
// state lock. Logical admission tests retry only that outcome within a deadline.
fn admit(client: &sdk::Client, data: Data, timestamp_ns: u64) -> sdk::Admission {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let result = client.try_record(data.clone(), timestamp_ns);
        if result != sdk::Admission::Contended || Instant::now() >= deadline {
            return result;
        }
        std::thread::yield_now();
    }
}
fn sample(sequence: u64) -> Observation {
    Observation {
        version: 2,
        registry: observation::REGISTRY.into(),
        identity: Identity {
            producer: "fixture".into(),
            generation: "generation".into(),
            sequence,
        },
        timestamp_ns: 100,
        resource: "test".into(),
        data: Data::Log {
            level: Level::Info,
            message: "independent fixture".into(),
        },
    }
}
fn bytes(sequence: u64) -> Vec<u8> {
    serde_json::to_vec(&sample(sequence)).unwrap()
}
fn config(root: &Path) -> spindle::Config {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    spindle::Config {
        socket: root.join("spindle.sock"),
        storage: root.join("custody"),
        quota_bytes: 2 * 1024 * 1024,
        max_connections: 8,
        client_timeout_ms: 100,
        files: vec![],
        journal: None,
        host: None,
    }
}
struct Running {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Running {
    fn start(config: spindle::Config) -> Self {
        let mut daemon = spindle::Daemon::open(config).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let signal = stop.clone();
        let thread = std::thread::spawn(move || {
            while !signal.load(Ordering::Relaxed) {
                daemon.tick().unwrap();
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap()
    }
}
#[test]
fn wire_validation_is_strict_and_legacy_explicit() {
    let valid=br#"{"version":2,"registry":"fabrico11y/2","identity":{"producer":"app","generation":"boot","sequence":1},"timestamp_ns":12,"resource":"host","data":{"kind":"metric","name":"requests","unit":"{request}","value":2.0,"metric_type":"counter","reset":true}}"#;
    assert!(observation::decode(valid).is_ok());
    assert!(observation::read_legacy(valid).is_err());
    for (from, to) in [
        ("\"version\":2", "\"version\":1"),
        ("\"value\":2.0", "\"value\":-1.0"),
        ("\"unit\":\"{request}\"", "\"unit\":\"\""),
        ("\"sequence\":1", "\"sequence\":0"),
        ("\"kind\":\"metric\"", "\"kind\":\"future\""),
    ] {
        assert!(
            observation::decode(
                String::from_utf8(valid.to_vec())
                    .unwrap()
                    .replace(from, to)
                    .as_bytes()
            )
            .is_err()
        );
    }
    assert!(observation::decode(&vec![b' '; 65537]).is_err());
    assert!(
        observation::check_json(
            format!("{}0{}", "[".repeat(17), "]".repeat(17)).as_bytes(),
            65536
        )
        .is_err()
    );
    let mut o = sample(1);
    o.data = Data::Metric {
        name: "nan".into(),
        unit: "1".into(),
        value: f64::NAN,
        metric_type: MetricType::Gauge,
        reset: false,
    };
    assert!(o.validate().is_err());
    let mut v = serde_json::to_value(sample(1)).unwrap();
    v["extra"] = true.into();
    assert!(observation::decode(&serde_json::to_vec(&v).unwrap()).is_err());
}
#[test]
fn durable_retry_conflict_and_checkpoint_are_atomic() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("store");
    let cp = Checkpoint {
        source: "source".into(),
        position: "offset 19".into(),
    };
    let mut store = Store::open(&path, 2 * 1024 * 1024).unwrap();
    let receipt = store.submit(&bytes(1), Some(&cp)).unwrap();
    assert!(!receipt.duplicate);
    assert_eq!(
        store.checkpoint("source").unwrap().as_deref(),
        Some("offset 19")
    );
    assert!(Store::open(&path, 2 * 1024 * 1024).is_err());
    drop(store);
    let mut store = Store::open(&path, 2 * 1024 * 1024).unwrap();
    assert!(store.submit(&bytes(1), None).unwrap().duplicate);
    let mut conflict = sample(1);
    conflict.timestamp_ns = 101;
    assert!(
        store
            .submit(
                &serde_json::to_vec(&conflict).unwrap(),
                Some(&Checkpoint {
                    source: "source".into(),
                    position: "bad".into()
                })
            )
            .unwrap_err()
            .contains("conflict")
    );
    assert_eq!(store.count().unwrap(), 1);
    assert_eq!(
        store.checkpoint("source").unwrap().as_deref(),
        Some("offset 19")
    );
}
#[test]
fn quota_refuses_without_cursor_or_record_and_read_stays_available() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("store");
    let quota = 1024 * 1024;
    let mut store = Store::open(&path, quota).unwrap();
    let mut accepted = 0;
    for sequence in 1..1000 {
        let mut o = sample(sequence);
        o.data = Data::Log {
            level: Level::Info,
            message: "a".repeat(16000),
        };
        let cp = Checkpoint {
            source: "file".into(),
            position: sequence.to_string(),
        };
        match store.submit(&serde_json::to_vec(&o).unwrap(), Some(&cp)) {
            Ok(_) => accepted = sequence,
            Err(_) => break,
        }
    }
    assert!(accepted > 0 && accepted < 1000);
    assert_eq!(store.count().unwrap(), accepted);
    assert_eq!(
        store.checkpoint("file").unwrap(),
        Some(accepted.to_string())
    );
    assert!(store.disk_bytes().unwrap() < quota);
    assert_eq!(store.read(0, 1).unwrap().len(), 1);
    drop(store);
    let store = Store::open(&path, quota).unwrap();
    assert_eq!(store.count().unwrap(), accepted);
}
#[test]
fn sdk_charges_until_custody_then_flushes_and_retries_after_outage() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let mut sdk_config = sdk::Config::local(config.socket.clone(), "test-sdk".into()).unwrap();
    sdk_config.slots = 2;
    sdk_config.delivery_timeout = Duration::from_millis(50);
    let client = sdk::Client::new(sdk_config).unwrap();
    for sequence in 1..=2 {
        assert_eq!(
            admit(
                &client,
                Data::Event {
                    name: "accepted".into(),
                    message: "bounded".into()
                },
                1
            ),
            sdk::Admission::Accepted { sequence }
        );
    }
    assert_eq!(
        admit(
            &client,
            Data::Event {
                name: "full".into(),
                message: "full".into()
            },
            1
        ),
        sdk::Admission::Full
    );
    assert_eq!(client.status().charged_slots, 2);
    assert!(
        client
            .flush(Instant::now() + Duration::from_millis(10))
            .is_err()
    );
    let daemon = Running::start(config.clone());
    let receipt = client
        .flush(Instant::now() + Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert_eq!(receipt.identity.sequence, 2);
    assert_eq!(client.status().charged_slots, 0);
    drop(daemon);
    let store = Store::open(&config.storage, config.quota_bytes).unwrap();
    assert_eq!(store.count().unwrap(), 2);
}
#[test]
fn lost_ack_retry_is_idempotent_and_malformed_peers_do_not_stall_status() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let daemon = Running::start(config.clone());
    {
        let mut stream = UnixStream::connect(&config.socket).unwrap();
        let b = bytes(1);
        stream.write_all(&(b.len() as u32).to_be_bytes()).unwrap();
        stream.write_all(&b).unwrap();
    }
    std::thread::sleep(Duration::from_millis(40));
    let r = spindle::transport::submit_bytes(
        &config.socket,
        &bytes(1),
        Instant::now() + Duration::from_secs(1),
    )
    .unwrap();
    assert!(r.duplicate);
    let mut slow = UnixStream::connect(&config.socket).unwrap();
    slow.write_all(&[0]).unwrap();
    let mut big = UnixStream::connect(&config.socket).unwrap();
    big.write_all(&u32::MAX.to_be_bytes()).unwrap();
    assert!(matches!(
        spindle::transport::request(
            &config.socket,
            &spindle::Request::Status,
            Instant::now() + Duration::from_secs(1)
        )
        .unwrap(),
        spindle::Reply::Status { .. }
    ));
    slow.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    let mut byte = [0];
    assert_eq!(slow.read(&mut byte).unwrap(), 0);
    drop(daemon);
    let daemon = Running::start(config.clone());
    assert!(
        spindle::transport::submit_bytes(
            &config.socket,
            &bytes(1),
            Instant::now() + Duration::from_secs(1)
        )
        .unwrap()
        .duplicate
    );
    drop(daemon);
}
#[test]
fn file_partial_rotation_truncation_and_restart() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("app.log");
    fs::write(&path, "old\n").unwrap();
    let mut store = Store::open(&temp.path().join("store"), 4 * 1024 * 1024).unwrap();
    let config = FileConfig {
        id: "app".into(),
        path: path.clone(),
        format: FileFormat::Text,
        replay: false,
    };
    let mut source = FileSource::new(config.clone());
    source.poll(&mut store);
    assert_eq!(store.count().unwrap(), 1);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"partial")
        .unwrap();
    source.poll(&mut store);
    assert_eq!(store.count().unwrap(), 1);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b" line\n")
        .unwrap();
    source.poll(&mut store);
    assert_eq!(store.count().unwrap(), 2);
    fs::rename(&path, temp.path().join("app.log.1")).unwrap();
    fs::write(&path, "new\n").unwrap();
    source.poll(&mut store);
    assert_eq!(store.count().unwrap(), 4);
    fs::write(&path, "x\n").unwrap();
    source.poll(&mut store);
    assert_eq!(store.count().unwrap(), 6);
    drop(source);
    let mut source = FileSource::new(config);
    source.poll(&mut store);
    assert_eq!(store.count().unwrap(), 6);
    let records = store.read(0, 100).unwrap();
    let messages: Vec<_> = records
        .iter()
        .filter_map(|(_, o)| match &o.data {
            Data::Log { message, .. } => Some(message.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(messages, ["partial line", "new", "x"]);
}
#[test]
fn jsonl_accepts_structured_logs_and_rejects_invalid_without_stalling() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("app.jsonl");
    fs::write(
        &path,
        "{\"message\":\"hi\",\"count\":3}\nnot json\n{\"ok\":true}\n",
    )
    .unwrap();
    let mut store = Store::open(&temp.path().join("store"), 2 * 1024 * 1024).unwrap();
    let mut source = FileSource::new(FileConfig {
        id: "app".into(),
        path,
        format: FileFormat::Jsonl,
        replay: true,
    });
    source.poll(&mut store);
    let records = store.read(0, 10).unwrap();
    assert_eq!(records.len(), 4);
    assert!(matches!(records[1].1.data, Data::JsonLog { .. }));
    assert!(matches!(records[2].1.data, Data::Gap { .. }));
    assert!(matches!(records[3].1.data, Data::JsonLog { .. }));
}
#[test]
fn adapter_is_persistent_and_machine_readable() {
    use std::process::{Command, Stdio};
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let _daemon = Running::start(config.clone());
    let mut child = Command::new(env!("CARGO_BIN_EXE_fabric-adapter"))
        .arg(&config.socket)
        .arg("python")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"{\"command\":\"record\",\"data\":{\"kind\":\"tool_execution\",\"tool\":\"generated-tool\",\"run_id\":\"r1\",\"duration_ns\":3,\"status\":\"ok\"},\"timestamp_ns\":1}\n{\"command\":\"flush\",\"timeout_ms\":2000}\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let lines: Vec<serde_json::Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0]["admission"].get("accepted").is_some());
    assert_eq!(lines[1]["receipt"]["identity"]["sequence"], 1);
}
#[test]
fn unavailable_journal_and_bad_source_are_explicit_and_self_excluded() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(&temp.path().join("store"), 2 * 1024 * 1024).unwrap();
    let mut journal = spindle::sources::JournalSource::new(spindle::sources::JournalConfig {
        unit: "fabric-impossible-test-unit.service".into(),
        system: false,
        replay: false,
        include_self: false,
    });
    journal.poll(&mut store);
    assert!(!journal.status.state.is_empty());
    eprintln!("live journal capability: {}", journal.status.state);
    assert!(
        spindle::sources::JournalConfig {
            unit: "spindle.service".into(),
            system: false,
            replay: false,
            include_self: false
        }
        .validate()
        .is_err()
    );
    let mut file = FileSource::new(FileConfig {
        id: "missing".into(),
        path: temp.path().join("missing"),
        format: FileFormat::Text,
        replay: true,
    });
    file.poll(&mut store);
    assert!(file.status.errors > 0);
    assert_eq!(store.checkpoint("file:missing").unwrap(), None);
}
#[test]
fn hostile_socket_paths_and_live_daemon_are_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    fs::write(&config.socket, "user data").unwrap();
    assert!(spindle::Daemon::open(config.clone()).is_err());
    assert_eq!(fs::read_to_string(&config.socket).unwrap(), "user data");
    fs::remove_file(&config.socket).unwrap();
    let daemon = spindle::Daemon::open(config.clone()).unwrap();
    let mut second = config.clone();
    second.storage = temp.path().join("other");
    assert!(spindle::Daemon::open(second).is_err());
    assert!(config.socket.exists());
    drop(daemon);
}
#[test]
fn generated_vocabulary_matches_serialized_types() {
    let kinds = [
        Data::ServiceRequest {
            service: "s".into(),
            operation: "o".into(),
            duration_ns: 1,
            status: observation::Outcome::Ok,
            trace_id: None,
        },
        Data::AgentRun {
            agent: "a".into(),
            run_id: "r".into(),
            duration_ns: 1,
            status: observation::Outcome::Error,
        },
        Data::ToolExecution {
            tool: "t".into(),
            run_id: "r".into(),
            duration_ns: 1,
            status: observation::Outcome::Cancelled,
        },
        Data::Log {
            level: Level::Info,
            message: "m".into(),
        },
        Data::JsonLog {
            fields: serde_json::from_str("{\"message\":\"m\"}").unwrap(),
        },
        Data::Event {
            name: "e".into(),
            message: "m".into(),
        },
        Data::Metric {
            name: "m".into(),
            unit: "1".into(),
            value: 1.,
            metric_type: MetricType::Counter,
            reset: true,
        },
        Data::Gap {
            source: "s".into(),
            reason: "r".into(),
        },
    ];
    for (data, expected) in kinds.iter().zip(observation::vocabulary::KIND) {
        assert_eq!(serde_json::to_value(data).unwrap()["kind"], *expected)
    }
    assert_eq!(kinds.len(), observation::vocabulary::KIND.len());
    for (value, expected) in [MetricType::Counter, MetricType::Gauge]
        .iter()
        .zip(observation::vocabulary::METRIC_TYPE)
    {
        assert_eq!(serde_json::to_value(value).unwrap(), *expected)
    }
    for (value, expected) in [
        observation::Outcome::Ok,
        observation::Outcome::Error,
        observation::Outcome::Cancelled,
    ]
    .iter()
    .zip(observation::vocabulary::OUTCOME)
    {
        assert_eq!(serde_json::to_value(value).unwrap(), *expected)
    }
}
#[test]
fn close_has_bounded_latency_during_outage_and_failed_encode_keeps_sequence() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = sdk::Config::local(temp.path().join("absent"), "bounded".into()).unwrap();
    config.slots = 1;
    config.max_record = 256;
    let client = sdk::Client::new(config).unwrap();
    assert_eq!(
        admit(
            &client,
            Data::Log {
                level: Level::Info,
                message: "x".repeat(1000)
            },
            1
        ),
        sdk::Admission::TooLarge
    );
    assert_eq!(client.status().accepted, 0);
    assert_eq!(client.status().charged_slots, 0);
    let start = Instant::now();
    drop(client);
    assert!(start.elapsed() < Duration::from_millis(50));
}
#[test]
fn maximum_valid_record_remains_readable_over_framed_response() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let _daemon = Running::start(config.clone());
    let mut value = sample(1);
    value.data = Data::Log {
        level: Level::Info,
        message: "\n".repeat(32000),
    };
    let b = serde_json::to_vec(&value).unwrap();
    assert!(b.len() > 64000 && b.len() < observation::MAX_FRAME);
    spindle::transport::submit_bytes(&config.socket, &b, Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert!(
        matches!(spindle::transport::request(&config.socket,&spindle::Request::Read{after:0,limit:1},Instant::now()+Duration::from_secs(1)).unwrap(),spindle::Reply::Records{records}if records.len()==1)
    );
}

#[test]
fn sdk_rejects_json_nesting_that_would_be_invalid_on_wire() {
    let temp = tempfile::tempdir().unwrap();
    let client =
        sdk::Client::new(sdk::Config::local(temp.path().join("absent"), "nested".into()).unwrap())
            .unwrap();
    let mut v = serde_json::json!(true);
    for _ in 0..15 {
        v = serde_json::json!({"child":v})
    }
    let mut fields = serde_json::Map::new();
    fields.insert("nested".into(), v);
    assert_eq!(
        admit(&client, Data::JsonLog { fields }, 1),
        sdk::Admission::Invalid
    );
    assert_eq!(client.status().accepted, 0);
}
#[test]
fn file_symlinks_are_refused() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(&temp.path().join("store"), 1024 * 1024).unwrap();
    let path = temp.path().join("source");
    fs::write(&path, "hello\n").unwrap();
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(path, &link).unwrap();
    let mut source = FileSource::new(FileConfig {
        id: "symlink".into(),
        path: link,
        format: FileFormat::Text,
        replay: true,
    });
    source.poll(&mut store);
    assert!(source.status.errors > 0);
    assert_eq!(store.count().unwrap(), 0);
}
#[test]
fn flush_completes_its_frontier_while_other_producers_continue() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let _daemon = Running::start(config.clone());
    let mut options = sdk::Config::local(config.socket, "concurrent".into()).unwrap();
    options.slots = 8;
    let client = Arc::new(sdk::Client::new(options).unwrap());
    assert!(matches!(
        admit(
            &client,
            Data::Event {
                name: "first".into(),
                message: "boundary".into()
            },
            1
        ),
        sdk::Admission::Accepted { .. }
    ));
    let stop = Arc::new(AtomicBool::new(false));
    let writer_client = client.clone();
    let writer_stop = stop.clone();
    let writer = std::thread::spawn(move || {
        while !writer_stop.load(Ordering::Relaxed) {
            let _ = writer_client.try_record(
                Data::Event {
                    name: "later".into(),
                    message: "ongoing".into(),
                },
                2,
            );
            std::thread::sleep(Duration::from_micros(100))
        }
    });
    let result = client.flush(Instant::now() + Duration::from_secs(2));
    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap();
    assert!(result.is_ok());
}
