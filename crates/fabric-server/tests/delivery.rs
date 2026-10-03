//! End-to-end delivery over TLS between a real node spool and a real server.

use fabric_o11y::spindle::host::Paths;
use fabric_o11y::spindle::runtime::{Config as NodeConfig, Spindle};
use fabric_o11y::spindle::sender::{Delivery, Sender, ServerTarget};
use fabric_o11y::spindle::spool::{Batch, Spool};
use fabric_server::config::Config;
use fabric_server::control::{Control, DesiredConfig};
use fabric_server::store::{CommitMode, Store};
use prost::Message;
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
            .join(format!("delivery-test-{}-{id}", std::process::id()));
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
    let status = Command::new("openssl")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("openssl is required for TLS tests");
    assert!(
        status.status.success(),
        "openssl {args:?}: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

/// Throwaway CA and a server certificate for 127.0.0.1, under the scratch root.
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

fn sha_hex(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
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

fn server_config(root: &Path) -> Config {
    Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        tls_cert: root.join("server.pem"),
        tls_key: root.join("server.key"),
        state_dir: root.join("server-state"),
        admin_token_file: root.join("admin-token"),
        journal_bytes: 64 * 1024 * 1024,
        journal_file_bytes: 64 * 1024 * 1024,
        retention_s: 86400,
        retention_bytes: 1 << 30,
        query_plan: fabric_server::query::Plan::Scan,
        seal_workers: 2,
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
        .expect("server did not start listening");
    Running {
        handle,
        thread: Some(thread),
        addr,
    }
}

fn target(root: &Path, addr: SocketAddr, token_name: &str) -> ServerTarget {
    ServerTarget {
        url: format!("https://{addr}"),
        ca: root.join("ca.pem"),
        token_file: root.join(token_name),
    }
}

fn write_host(root: &Path) {
    fs::write(root.join("stat"), "cpu 10 0 5 20 0 0 0 0 0 0\nbtime 1000\n").unwrap();
    fs::write(
        root.join("meminfo"),
        "MemTotal: 1000 kB\nMemAvailable: 500 kB\n",
    )
    .unwrap();
    fs::write(root.join("diskstats"), "8 0 sda 1 0 4 0 1 0 8 0\n").unwrap();
    fs::write(
        root.join("netdev"),
        "h\nh\neth0: 10 0 0 0 0 0 0 0 20 0 0 0 0 0 0 0\n",
    )
    .unwrap();
    fs::write(root.join("boot_id"), "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();
    fs::write(root.join("hostname"), "fixture-host\n").unwrap();
}

fn host_paths(root: &Path) -> Paths {
    Paths {
        proc_stat: root.join("stat"),
        proc_meminfo: root.join("meminfo"),
        proc_diskstats: root.join("diskstats"),
        proc_net_dev: root.join("netdev"),
        boot_id: root.join("boot_id"),
        hostname: root.join("hostname"),
        filesystem: root.to_owned(),
    }
}

fn setup() -> Scratch {
    let scratch = Scratch::new();
    make_certs(&scratch.0);
    fs::write(
        scratch.path("admin-token"),
        format!("{}\n", sha_hex("admin")),
    )
    .unwrap();
    fs::write(scratch.path("token-bad"), "not-enrolled\n").unwrap();
    // Enroll before the server starts; the server reads the same state.
    let mut control = Control::open(&server_config(&scratch.0).state_dir).unwrap();
    for name in ["node-a", "node-b"] {
        let desired = DesiredConfig {
            logs: vec![scratch.path("app.log").to_string_lossy().into_owned()],
            metric_interval_s: 15,
        };
        let (_, token) = control.enroll(name, desired).unwrap();
        let file = if name == "node-a" {
            "token-a"
        } else {
            "token-b"
        };
        fs::write(scratch.path(file), format!("{token}\n")).unwrap();
    }
    write_host(&scratch.0);
    scratch
}

fn recovered(root: &Path) -> Vec<Vec<u8>> {
    let config = server_config(root);
    let mut out = Vec::new();
    Store::replay(&config.state_dir, config.journal_bytes, |entry| {
        out.push(entry.batch.clone());
        Ok(())
    })
    .unwrap();
    out
}

fn spool_bytes(spool: &Path) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    Spool::inspect(spool, 16 * 1024 * 1024, |batch| {
        out.push(batch.encode_to_vec());
        Ok(())
    })
    .unwrap();
    out
}

#[test]
fn node_delivers_exact_bytes_and_both_sides_survive_restart() {
    let scratch = setup();
    fs::write(scratch.path("app.log"), "one\ntwo\n").unwrap();
    let server = start(server_config(&scratch.0));
    let node_config = |addr| NodeConfig {
        spool: scratch.path("spool"),
        logs: vec![scratch.path("app.log")],
        interval_s: 15,
        spool_bytes: 16 * 1024 * 1024,
        server: Some(target(&scratch.0, addr, "token-a")),
        traces_listen: None,
        max_output_bytes_per_s: None,
    };
    let mut node =
        Spindle::open_with_paths(node_config(server.addr), host_paths(&scratch.0)).unwrap();
    for _ in 0..3 {
        node.collect_once().unwrap();
    }
    let report = node
        .deliver(Instant::now() + Duration::from_secs(20), |_| {})
        .unwrap();
    assert!(report.caught_up, "{report:?}");
    assert_eq!((report.sent, report.acked_through), (3, 3));
    drop(node);
    server.stop();
    assert_eq!(recovered(&scratch.0), spool_bytes(&scratch.path("spool")));

    // Restart both; the node resumes after its durable ACK cursor.
    let server = start(server_config(&scratch.0));
    fs::write(scratch.path("app.log"), "one\ntwo\nthree\n").unwrap();
    let mut node =
        Spindle::open_with_paths(node_config(server.addr), host_paths(&scratch.0)).unwrap();
    assert_eq!(node.acked_through(), 3);
    node.collect_once().unwrap();
    let report = node
        .deliver(Instant::now() + Duration::from_secs(20), |_| {})
        .unwrap();
    assert_eq!((report.sent, report.acked_through), (1, 4));
    drop(node);
    server.stop();
    assert_eq!(recovered(&scratch.0), spool_bytes(&scratch.path("spool")));
}

/// Real batches with this spool's identity, as stored bytes.
fn batches(spool: &Path, count: usize) -> Vec<Vec<u8>> {
    let mut journal = Spool::open(spool, 16 * 1024 * 1024).unwrap();
    let template = Batch {
        version: 1,
        node_id: vec![],
        generation: 0,
        sequence: 0,
        metrics: vec![],
        logs: vec![],
        cursors: vec![],
        collection_gaps: vec!["fixture".into()],
        traces: Vec::new(),
    };
    (0..count)
        .map(|_| journal.append(&template).unwrap().encode_to_vec())
        .collect()
}

#[test]
fn retry_conflict_gap_binding_and_rejections_follow_the_delivery_rule() {
    let scratch = setup();
    let server = start(server_config(&scratch.0));
    let a = Sender::new(&target(&scratch.0, server.addr, "token-a")).unwrap();
    let b = Sender::new(&target(&scratch.0, server.addr, "token-b")).unwrap();
    let bad = Sender::new(&target(&scratch.0, server.addr, "token-bad")).unwrap();
    let stream_a = batches(&scratch.path("spool-a"), 3);
    let stream_b = batches(&scratch.path("spool-b"), 1);

    assert!(
        matches!(bad.send(&stream_a[0]), Delivery::Rejected(ref w) if w.starts_with("HTTP 401"))
    );
    assert!(
        matches!(a.send(b"not a batch"), Delivery::Rejected(ref w) if w.starts_with("HTTP 400"))
    );
    assert_eq!(a.send(&stream_a[1]), Delivery::Gap(0));
    assert_eq!(a.send(&stream_a[0]), Delivery::Ack(1));
    // A lost ACK: the same identity and bytes again is one logical commit.
    assert_eq!(a.send(&stream_a[0]), Delivery::Ack(1));
    // The same identity with different bytes is refused and not replaced.
    let mut changed = Batch::decode(stream_a[0].as_slice()).unwrap();
    changed.collection_gaps = vec!["different".into()];
    assert_eq!(a.send(&changed.encode_to_vec()), Delivery::Conflict(1));
    assert_eq!(a.send(&stream_a[1]), Delivery::Ack(2));
    // A lagging retry of an older sequence acknowledges the committed prefix.
    assert_eq!(a.send(&stream_a[0]), Delivery::Ack(2));
    // Credential label and node identity bind one-to-one on first commit.
    assert!(matches!(a.send(&stream_b[0]), Delivery::Rejected(ref w) if w.starts_with("HTTP 403")));
    assert_eq!(b.send(&stream_b[0]), Delivery::Ack(1));
    assert!(matches!(b.send(&stream_a[2]), Delivery::Rejected(ref w) if w.starts_with("HTTP 403")));
    let oversized = vec![0_u8; 1024 * 1024 + 2];
    assert!(matches!(a.send(&oversized), Delivery::Rejected(ref w) if w.starts_with("HTTP 413")));
    server.stop();

    let recovered = recovered(&scratch.0);
    assert_eq!(
        recovered,
        vec![
            stream_a[0].clone(),
            stream_a[1].clone(),
            stream_b[0].clone()
        ]
    );
}

/// ADR-0025 end to end: an application exports spans to the Spindle's loopback
/// endpoint, each export is answered only after its Spool commit, the trace Batches
/// reach the server byte for byte, and the spans query returns every span, the same
/// from the scan and the walk plan, and finds one trace by its ID.
#[test]
fn exported_spans_reach_the_server_and_answer_the_spans_query() {
    use fabric_o11y::spindle::otlp;
    use fabric_server::query::{History, Plan, Query};
    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
    use std::io::{BufRead, BufReader, Read, Write};
    let scratch = setup();
    let server = start(server_config(&scratch.0));
    let config = NodeConfig {
        spool: scratch.path("spool"),
        logs: vec![],
        interval_s: 15,
        spool_bytes: 16 * 1024 * 1024,
        server: Some(target(&scratch.0, server.addr, "token-a")),
        traces_listen: Some("127.0.0.1:0".parse().unwrap()),
        max_output_bytes_per_s: None,
    };
    let mut node = Spindle::open_with_paths(config.clone(), host_paths(&scratch.0)).unwrap();
    let (addr, rx) = otlp::start(config.traces_listen.unwrap()).unwrap();
    let export = |e: u64| {
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![ScopeSpans {
                    spans: (0..4)
                        .map(|i| Span {
                            trace_id: vec![e as u8; 16],
                            span_id: (e * 10 + i + 1).to_be_bytes().to_vec(),
                            parent_span_id: if i == 0 {
                                vec![]
                            } else {
                                (e * 10 + 1).to_be_bytes().to_vec()
                            },
                            name: format!("op-{i}"),
                            kind: 2,
                            start_time_unix_nano: 5_000 + e * 100 + i,
                            end_time_unix_nano: 5_050 + e * 100 + i,
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
        .encode_to_vec()
    };
    // The exporter: ten exports on one connection, each waiting for its answer.
    let client = std::thread::spawn(move || {
        let mut stream = std::net::TcpStream::connect(addr).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut statuses = Vec::new();
        for e in 1..=10 {
            let body = export(e);
            write!(stream, "POST /v1/traces HTTP/1.1\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\n\r\n", body.len()).unwrap();
            stream.write_all(&body).unwrap();
            let mut status = String::new();
            reader.read_line(&mut status).unwrap();
            let mut len = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            reader.read_exact(&mut vec![0; len]).unwrap();
            statuses.push(status.split(' ').nth(1).unwrap().to_owned());
        }
        statuses
    });
    // The Spindle's loop, reduced to the two calls that matter here.
    let mut carried = None;
    let deadline = Instant::now() + Duration::from_secs(30);
    while !client.is_finished() && Instant::now() < deadline {
        otlp::drain(&mut node, &rx, &mut carried);
        std::thread::sleep(Duration::from_millis(5));
    }
    let statuses = client.join().unwrap();
    assert_eq!(statuses, vec!["200"; 10]);
    let report = node
        .deliver(Instant::now() + Duration::from_secs(20), |_| {})
        .unwrap();
    assert!(report.caught_up, "{report:?}");
    drop(node);
    server.stop();
    assert_eq!(
        recovered(&scratch.0),
        spool_bytes(&scratch.path("spool")),
        "custody: exact bytes"
    );

    let state = server_config(&scratch.0).state_dir;
    let all: Query = serde_json::from_value(
        serde_json::json!({"kind": "spans", "from_ns": 0, "to_ns": 1u64 << 40, "limit": 1000}),
    )
    .unwrap();
    let scan = History::new(&state).run(&all, 1 << 40).unwrap();
    let walk = History::with_plan(&state, Plan::Walk)
        .run(&all, 1 << 40)
        .unwrap();
    assert_eq!(scan, walk);
    let rows = scan["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 40);
    assert_eq!(rows[0]["node"], "node-a");
    assert_eq!(rows[0]["start_ns"], 5_100);
    assert_eq!(rows[1]["parent_span_id"], rows[0]["span_id"]);
    let one: Query = serde_json::from_value(serde_json::json!({"kind": "spans", "from_ns": 0, "to_ns": 1u64 << 40, "trace_id": "07070707070707070707070707070707", "limit": 10})).unwrap();
    let trace = History::with_plan(&state, Plan::Walk)
        .run(&one, 1 << 40)
        .unwrap();
    let names: Vec<_> = trace["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["op-0", "op-1", "op-2", "op-3"]);
}

/// The Spindle's output cap delays delivery to the configured rate (a token bucket
/// with a one-Batch burst) and meters the time it waited.
#[test]
fn the_output_cap_holds_delivery_to_its_rate() {
    let scratch = setup();
    let server = start(server_config(&scratch.0));
    let rate = 512 * 1024;
    let mut config = NodeConfig {
        spool: scratch.path("spool"),
        logs: vec![],
        interval_s: 15,
        spool_bytes: 64 * 1024 * 1024,
        server: Some(target(&scratch.0, server.addr, "token-a")),
        traces_listen: None,
        max_output_bytes_per_s: Some(rate),
    };
    let mut node = Spindle::open_with_paths(config.clone(), host_paths(&scratch.0)).unwrap();
    // Eight trace Batches of about 480 KiB: 3.9 MB, of which 1 MiB is burst.
    let body = {
        use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
        use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![ScopeSpans {
                    spans: (0..2000_u64)
                        .map(|i| Span {
                            trace_id: vec![1; 16],
                            span_id: (i + 1).to_be_bytes().to_vec(),
                            name: format!("{i:0>200}"),
                            start_time_unix_nano: 10 + i,
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
        .encode_to_vec()
    };
    for _ in 0..8 {
        node.commit_traces(&[body.as_slice()]).unwrap();
    }
    let total: usize = spool_bytes(&scratch.path("spool"))
        .iter()
        .map(Vec::len)
        .sum();
    let started = Instant::now();
    loop {
        let report = node
            .deliver(Instant::now() + Duration::from_secs(2), |_| {})
            .unwrap();
        assert!(report.error.is_none(), "{report:?}");
        if report.caught_up {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(60));
    }
    let elapsed = started.elapsed().as_secs_f64();
    let floor = (total as f64 - (1 << 20) as f64) / rate as f64;
    assert!(
        elapsed >= floor * 0.95,
        "{total} bytes in {elapsed:.2} s, floor {floor:.2} s"
    );
    assert!(
        elapsed < floor + 5.0,
        "the cap must not stall delivery: {elapsed:.2} s"
    );
    drop(node);
    // Uncapped, the same history is already delivered; a fresh node reports the
    // waiting it did in its next metric cycle.
    config.max_output_bytes_per_s = None;
    server.stop();
}
