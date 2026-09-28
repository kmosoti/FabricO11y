//! Retained history end to end: sealing into segments, queries graded by the
//! independent Python oracle, crash recovery of sealing, retention, stream
//! checkpoints and incomplete answers.

use fabric_o11y::spindle::sender::{Delivery, Sender, ServerTarget, agent};
use fabric_o11y::spindle::spool::{Batch, Spool};
use fabric_server::config::Config;
use fabric_server::control::{Control, DesiredConfig};
use fabric_server::store::{CommitMode, Store};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum, metric, number_data_point,
};
use prost::Message;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!("history-test-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn openssl(args: &[&str], dir: &Path) {
    let out = Command::new("openssl")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn make_certs(dir: &Path) {
    openssl(
        &[
            "req",
            "-x509",
            "-newkey",
            "ec",
            "-pkeyopt",
            "ec_paramgen_curve:P-256",
            "-nodes",
            "-keyout",
            "ca.key",
            "-out",
            "ca.pem",
            "-days",
            "2",
            "-subj",
            "/CN=fabric test CA",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-addext",
            "keyUsage=critical,keyCertSign,cRLSign",
        ],
        dir,
    );
    openssl(
        &[
            "req",
            "-newkey",
            "ec",
            "-pkeyopt",
            "ec_paramgen_curve:P-256",
            "-nodes",
            "-keyout",
            "server.key",
            "-out",
            "server.csr",
            "-subj",
            "/CN=127.0.0.1",
        ],
        dir,
    );
    fs::write(dir.join("san.ext"), "subjectAltName=IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n").unwrap();
    openssl(
        &[
            "x509",
            "-req",
            "-in",
            "server.csr",
            "-CA",
            "ca.pem",
            "-CAkey",
            "ca.key",
            "-CAcreateserial",
            "-out",
            "server.pem",
            "-days",
            "2",
            "-extfile",
            "san.ext",
        ],
        dir,
    );
}

fn config(root: &Path, file_bytes: u64, retention_bytes: u64) -> Config {
    Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        tls_cert: root.join("server.pem"),
        tls_key: root.join("server.key"),
        state_dir: root.join("server-state"),
        admin_token_file: root.join("admin-token"),
        journal_bytes: 256 * 1024 * 1024,
        journal_file_bytes: file_bytes,
        retention_s: 86400,
        retention_bytes,
    }
}

struct Running {
    handle: axum_server::Handle,
    thread: Option<std::thread::JoinHandle<std::io::Result<()>>>,
    addr: SocketAddr,
}
impl Running {
    fn stop(mut self) {
        self.handle.graceful_shutdown(Some(Duration::from_secs(5)));
        self.thread.take().unwrap().join().unwrap().unwrap();
    }
}

fn start(config: Config) -> Running {
    let handle = axum_server::Handle::new();
    let serving = handle.clone();
    let thread = std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(fabric_server::serve(config, CommitMode::GROUPED, serving))
    });
    let addr = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(10), handle.listening())
                .await
                .ok()
                .flatten()
        })
        .expect("server did not listen");
    Running {
        handle,
        thread: Some(thread),
        addr,
    }
}

fn admin_token() -> String {
    Sha256::digest(b"admin")
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Enroll `names` before the server starts; returns token file paths.
fn enroll(scratch: &Scratch, names: &[&str]) -> Vec<PathBuf> {
    fs::write(scratch.path("admin-token"), format!("{}\n", admin_token())).unwrap();
    let mut control = Control::open(&scratch.path("server-state")).unwrap();
    names
        .iter()
        .map(|name| {
            let (_, token) = control
                .enroll(
                    name,
                    DesiredConfig {
                        logs: vec![],
                        metric_interval_s: 15,
                    },
                )
                .unwrap();
            let path = scratch.path(&format!("token-{name}"));
            fs::write(&path, format!("{token}\n")).unwrap();
            path
        })
        .collect()
}

fn kv(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.into())),
        }),
        ..Default::default()
    }
}

/// A batch with `n` log lines at `t0 + i` and one monotonic counter point.
fn payload(t0: u64, n: u64, counter: i64, start: u64) -> Batch {
    let logs = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: (0..n)
                    .map(|i| LogRecord {
                        observed_time_unix_nano: t0 + i,
                        body: Some(AnyValue {
                            value: Some(any_value::Value::StringValue(format!(
                                "line {t0}+{i} {}",
                                if i % 3 == 0 { "needle" } else { "hay" }
                            ))),
                        }),
                        attributes: vec![kv("log.file.path", "/var/log/app.log")],
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    let metrics = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            scope_metrics: vec![ScopeMetrics {
                metrics: vec![Metric {
                    name: "system.network.receive.bytes".into(),
                    unit: "By".into(),
                    data: Some(metric::Data::Sum(Sum {
                        data_points: vec![NumberDataPoint {
                            attributes: vec![kv("interface", "eth0")],
                            start_time_unix_nano: start,
                            time_unix_nano: t0,
                            value: Some(number_data_point::Value::AsInt(counter)),
                            ..Default::default()
                        }],
                        aggregation_temporality: 2,
                        is_monotonic: true,
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    Batch {
        version: 1,
        node_id: vec![],
        generation: 0,
        sequence: 0,
        metrics: metrics.encode_to_vec(),
        logs: logs.encode_to_vec(),
        cursors: vec![],
        collection_gaps: if t0.is_multiple_of(7) {
            vec![format!("gap at {t0}")]
        } else {
            vec![]
        },
    }
}

/// Commit `count` batches to a node spool and deliver them.
fn deliver(spool: &Path, sender: &Sender, first: u64, count: u64) {
    let mut journal = Spool::open(spool, 64 * 1024 * 1024).unwrap();
    for k in first..first + count {
        let start = if k < 20 { 1_000 } else { 5_000 }; // one counter reset
        let counter = (k as i64 % 20) * 1000 + 17;
        let batch = journal
            .append(&payload(1_000_000 + k * 100, 25, counter, start))
            .unwrap();
        let answer = sender.send(&batch.encode_to_vec());
        assert!(matches!(answer, Delivery::Ack(_)), "{answer:?}");
    }
}

fn sender(root: &Path, addr: SocketAddr, token: &Path) -> Sender {
    Sender::new(&ServerTarget {
        url: format!("https://{addr}"),
        ca: root.join("ca.pem"),
        token_file: token.to_path_buf(),
    })
    .unwrap()
}

fn query(root: &Path, addr: SocketAddr, body: &Value) -> (u16, Value) {
    let agent = agent(&root.join("ca.pem")).unwrap();
    let mut response = agent
        .post(&format!("https://{addr}/v1/admin/query"))
        .header("authorization", &format!("Bearer {}", admin_token()))
        .header("content-type", "application/json")
        .send(body.to_string())
        .unwrap();
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .with_config()
        .limit(64 << 20)
        .read_to_string()
        .unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// Every page of a query, following `next_page`.
fn all_pages(root: &Path, addr: SocketAddr, mut body: Value) -> Vec<Value> {
    let mut pages = Vec::new();
    loop {
        let (status, page) = query(root, addr, &body);
        assert_eq!(status, 200, "{page}");
        let next = page["next_page"].clone();
        pages.push(page);
        if next.is_null() {
            return pages;
        }
        body["page"] = next;
    }
}

fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |a, (i, b)| a | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                T[((n >> (18 - 6 * i)) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

/// Grade pages with the independent oracle over the server's retained records.
fn oracle(
    scratch: &Scratch,
    cfg: &Config,
    query: &Value,
    pages: &[Value],
    unavailable: Option<&Value>,
) -> Value {
    let mut records = String::new();
    Store::replay(&cfg.state_dir, cfg.journal_bytes, |entry| {
        records.push_str(&json!({"label": entry.label, "received_ns": entry.received_unix_nano, "bytes": b64(&entry.batch)}).to_string());
        records.push('\n');
        Ok(())
    })
    .unwrap();
    fs::write(scratch.path("records.jsonl"), records).unwrap();
    fs::write(scratch.path("query.json"), query.to_string()).unwrap();
    fs::write(
        scratch.path("answer.json"),
        Value::Array(pages.to_vec()).to_string(),
    )
    .unwrap();
    let mut cmd = Command::new("python3");
    cmd.arg("-B")
        .arg(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../tools/qualification/query_oracle.py"),
        )
        .arg("--records")
        .arg(scratch.path("records.jsonl"))
        .arg("--query")
        .arg(scratch.path("query.json"))
        .arg("--answer")
        .arg(scratch.path("answer.json"));
    if let Some(u) = unavailable {
        fs::write(scratch.path("unavailable.json"), u.to_string()).unwrap();
        cmd.arg("--unavailable")
            .arg(scratch.path("unavailable.json"));
    }
    let out = cmd.output().unwrap();
    serde_json::from_slice(&out.stdout).unwrap_or_else(|_| json!({"passed": false, "raw": String::from_utf8_lossy(&out.stdout), "err": String::from_utf8_lossy(&out.stderr)}))
}

fn wait_for_segments(state_dir: &Path, at_least: usize) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let segments = fabric_server::segment::list(state_dir).unwrap().len();
        let sealed = fs::read_dir(state_dir.join("journal"))
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("sealed-")
            })
            .count();
        if segments >= at_least && sealed == 0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("sealer did not produce {at_least} segments");
}

#[test]
fn sealed_history_answers_exactly_and_pages_are_stable() {
    let scratch = Scratch::new();
    make_certs(&scratch.0);
    let tokens = enroll(&scratch, &["node-a", "node-b"]);
    let cfg = config(&scratch.0, 16 * 1024, 1 << 30);
    let server = start(cfg.clone());
    let a = sender(&scratch.0, server.addr, &tokens[0]);
    let b = sender(&scratch.0, server.addr, &tokens[1]);
    deliver(&scratch.path("spool-a"), &a, 0, 40);
    deliver(&scratch.path("spool-b"), &b, 0, 40);
    wait_for_segments(&cfg.state_dir, 2);
    // Some records remain in the journal tail as well.
    deliver(&scratch.path("spool-a"), &a, 40, 3);

    let queries = [
        json!({"kind": "logs", "from_ns": 1_000_000, "to_ns": 1_004_000, "limit": 37}),
        json!({"kind": "logs", "node": "node-b", "from_ns": 1_000_000, "to_ns": 1_010_000, "contains": "needle", "limit": 50}),
        json!({"kind": "metrics", "name": "system.network.receive.bytes", "from_ns": 0, "to_ns": u64::MAX / 2, "limit": 9}),
    ];
    let mut graded = Vec::new();
    for q in &queries {
        graded.push((q.clone(), all_pages(&scratch.0, server.addr, q.clone())));
    }
    let rate = json!({"kind": "rate", "node": "node-a", "name": "system.network.receive.bytes", "from_ns": 0, "to_ns": u64::MAX / 2});
    let (status, rate_answer) = query(&scratch.0, server.addr, &rate);
    assert_eq!(status, 200);
    graded.push((rate, vec![rate_answer]));
    // A page token is bound to its query.
    let first = &graded[0].1[0];
    let mut wrong = queries[1].clone();
    wrong["page"] = first["next_page"].clone();
    assert_eq!(query(&scratch.0, server.addr, &wrong).0, 400);
    server.stop();

    for (q, pages) in &graded {
        let verdict = oracle(&scratch, &cfg, q, pages, None);
        assert_eq!(verdict["passed"], true, "query {q}: {verdict}");
    }
}

#[test]
fn crash_states_of_sealing_never_serve_a_record_twice() {
    let scratch = Scratch::new();
    make_certs(&scratch.0);
    let tokens = enroll(&scratch, &["node-a"]);
    let cfg = config(&scratch.0, 16 * 1024, 1 << 30);
    let server = start(cfg.clone());
    let a = sender(&scratch.0, server.addr, &tokens[0]);
    deliver(&scratch.path("spool-a"), &a, 0, 30);
    wait_for_segments(&cfg.state_dir, 1);
    server.stop();
    let before = {
        let mut n = 0;
        Store::replay(&cfg.state_dir, cfg.journal_bytes, |_| {
            n += 1;
            Ok(())
        })
        .unwrap();
        n
    };
    assert_eq!(before, 30);
    let segments = cfg.state_dir.join("segments");
    let (label, _) = fabric_server::segment::list(&cfg.state_dir).unwrap()[0].clone();
    // State A: killed while building: a leftover .building directory.
    fs::create_dir_all(segments.join(".building-00000000000000000099")).unwrap();
    fs::write(
        segments.join(".building-00000000000000000099/logs.parquet"),
        b"partial",
    )
    .unwrap();
    // State B: segment committed, journal file not yet reclaimed. Rebuild the
    // sealed file from the segment's own records to reproduce that state.
    let dir = segments.join(fabric_server::segment::segment_name(label));
    let manifest = fabric_server::segment::read_manifest(&dir).unwrap();
    let mut groups: std::collections::BTreeMap<u64, Vec<fabric_server::store::Entry>> =
        Default::default();
    fabric_server::segment::scan_batches(&dir, &manifest, |g, e| {
        groups.entry(g).or_default().push(e)
    })
    .unwrap();
    let journal = cfg.state_dir.join("journal");
    fs::create_dir_all(scratch.path("rebuild")).unwrap();
    let mut log =
        fabric_frame::frame::FrameLog::open(&scratch.path("rebuild"), 1 << 30, 4 << 20, |_, _| {
            Ok(())
        })
        .unwrap();
    for (g, entries) in &groups {
        log.append(
            &fabric_server::store::Group {
                group_sequence: *g,
                entries: entries.clone(),
            }
            .encode_to_vec(),
        )
        .unwrap();
    }
    drop(log);
    fs::copy(
        scratch.path("rebuild/batches.faj"),
        journal.join(format!("sealed-{label:020}.faj")),
    )
    .unwrap();
    // A query in the live window between segment commit and reclaim sees
    // each record once, although both copies exist.
    let history = fabric_server::query::History::new(&cfg.state_dir);
    let all = serde_json::from_value(
        json!({"kind": "logs", "from_ns": 0, "to_ns": u64::MAX / 2, "limit": 10000}),
    )
    .unwrap();
    let live = history
        .run(&all, *groups.keys().max().unwrap() + 1000)
        .unwrap();
    assert_eq!(live["rows"].as_array().unwrap().len(), 30 * 25);
    // Restart: the leftover build is removed and the duplicate journal file
    // is reclaimed before serving.
    let server = start(cfg.clone());
    let (status, answer) = query(
        &scratch.0,
        server.addr,
        &json!({"kind": "logs", "from_ns": 0, "to_ns": u64::MAX / 2, "limit": 10000}),
    );
    assert_eq!(status, 200);
    assert_eq!(answer["rows"].as_array().unwrap().len(), 30 * 25);
    assert!(!segments.join(".building-00000000000000000099").exists());
    assert!(!journal.join(format!("sealed-{label:020}.faj")).exists());
    server.stop();
}

#[test]
fn retention_removes_old_segments_and_stream_state_survives_it() {
    let scratch = Scratch::new();
    make_certs(&scratch.0);
    let tokens = enroll(&scratch, &["node-a"]);
    // Keep about one segment.
    let cfg = config(&scratch.0, 16 * 1024, 20 * 1024);
    let server = start(cfg.clone());
    let a = sender(&scratch.0, server.addr, &tokens[0]);
    deliver(&scratch.path("spool-a"), &a, 0, 25);
    let first_page = query(
        &scratch.0,
        server.addr,
        &json!({"kind": "logs", "from_ns": 0, "to_ns": u64::MAX / 2, "limit": 5}),
    )
    .1;
    deliver(&scratch.path("spool-a"), &a, 25, 60);
    wait_for_segments(&cfg.state_dir, 1);
    std::thread::sleep(Duration::from_secs(3)); // at least one retention pass
    let segments = fabric_server::segment::list(&cfg.state_dir).unwrap();
    let total: u64 = segments
        .iter()
        .map(|(_, m)| m.files.values().map(|f| f.bytes).sum::<u64>())
        .sum();
    assert!(total <= 20 * 1024, "retention left {total} bytes");
    assert!(
        segments.first().unwrap().1.first_group > 1,
        "oldest segment was not deleted"
    );
    // A page bound to deleted history is Gone.
    let mut next = json!({"kind": "logs", "from_ns": 0, "to_ns": u64::MAX / 2, "limit": 5});
    next["page"] = first_page["next_page"].clone();
    assert_eq!(query(&scratch.0, server.addr, &next).0, 410);
    let (_, fresh) = query(
        &scratch.0,
        server.addr,
        &json!({"kind": "logs", "from_ns": 0, "to_ns": u64::MAX / 2, "limit": 1}),
    );
    assert!(
        fresh["retained_from_ns"].as_u64().unwrap()
            > first_page["retained_from_ns"].as_u64().unwrap()
    );
    server.stop();
    // Every early batch left the journal; a restarted server still knows the
    // stream and acknowledges the next sequence instead of reporting a gap.
    let server = start(cfg.clone());
    let a = sender(&scratch.0, server.addr, &tokens[0]);
    deliver(&scratch.path("spool-a"), &a, 85, 1);
    server.stop();
}

#[test]
fn a_corrupt_segment_makes_the_answer_incomplete() {
    let scratch = Scratch::new();
    make_certs(&scratch.0);
    let tokens = enroll(&scratch, &["node-a"]);
    let cfg = config(&scratch.0, 16 * 1024, 1 << 30);
    let server = start(cfg.clone());
    let a = sender(&scratch.0, server.addr, &tokens[0]);
    deliver(&scratch.path("spool-a"), &a, 0, 30);
    wait_for_segments(&cfg.state_dir, 1);
    let (label, _) = fabric_server::segment::list(&cfg.state_dir).unwrap()[0].clone();
    let logs = cfg
        .state_dir
        .join("segments")
        .join(fabric_server::segment::segment_name(label))
        .join("logs.parquet");
    let mut bytes = fs::read(&logs).unwrap();
    let middle = bytes.len() / 2;
    bytes.truncate(middle);
    fs::write(&logs, bytes).unwrap();
    let (status, answer) = query(
        &scratch.0,
        server.addr,
        &json!({"kind": "logs", "from_ns": 0, "to_ns": u64::MAX / 2, "limit": 10000}),
    );
    assert_eq!(status, 200);
    assert_eq!(answer["complete"], false);
    assert_eq!(answer["unavailable"].as_array().unwrap().len(), 1);
    server.stop();
}
