//! Finite actual collector/TLS/restart/publication regression, not capacity.
use fabric_frame::frame::FrameLog;
use fabric_o11y::spindle::host::Paths;
use fabric_o11y::spindle::runtime::{Config as NodeConfig, Spindle};
use fabric_o11y::spindle::sender::{self, Delivery, Sender, ServerTarget};
use fabric_o11y::spindle::spool::Batch;
use fabric_server::{
    config::Config,
    control::{Control, DesiredConfig},
    query::Plan,
    store::{CommitMode, Entry, Store},
};
use prost::Message;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    cell::Cell,
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
    time::{Duration, Instant},
};
static DEADLINE: OnceLock<Instant> = OnceLock::new();
const ADMIN: &str = "native-lifecycle-admin-0123456789abcdef0123456789abcdef";
const MIB: u64 = 1024 * 1024;
fn check_time() {
    assert!(
        Instant::now() < *DEADLINE.get().unwrap(),
        "native lifecycle240s deadline"
    );
}
fn bounded_command(mut command: Command, label: &str, root: &Path, seconds: u64) {
    check_time();
    let stdout = root.join(format!("{label}.stdout"));
    let stderr = root.join(format!("{label}.stderr"));
    fs::write(root.join(format!("{label}.command.json")),json!({"program":command.get_program().to_string_lossy(),
        "args":command.get_args().map(|a|a.to_string_lossy().to_string()).collect::<Vec<_>>(),"timeout_s":seconds}).to_string()).unwrap();
    let mut child = command
        .stdout(File::create(stdout).unwrap())
        .stderr(File::create(stderr).unwrap())
        .spawn()
        .unwrap();
    let until = (*DEADLINE.get().unwrap()).min(Instant::now() + Duration::from_secs(seconds));
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= until {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("{label} timeout; evidence preserved");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    fs::write(
        root.join(format!("{label}.exit.json")),
        json!({"code":status.code(),"success":status.success()}).to_string(),
    )
    .unwrap();
    assert!(status.success(), "{label} failed; evidence preserved");
}
fn openssl(args: &[&str], dir: &Path) {
    let label = format!("openssl-{}", fs::read_dir(dir).unwrap().count());
    let mut command = Command::new("openssl");
    command.args(args).current_dir(dir);
    bounded_command(command, &label, dir, 20);
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

struct Scratch {
    root: PathBuf,
    next: Cell<usize>,
    complete: bool,
}
impl Scratch {
    fn new() -> Self {
        DEADLINE
            .set(Instant::now() + Duration::from_secs(240))
            .unwrap();
        let root =
            PathBuf::from(std::env::var("FABRIC_SCRATCH_ROOT").expect("contained mounted scratch"))
                .join(format!("native-lifecycle-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        Self {
            root,
            next: Cell::new(0),
            complete: false,
        }
    }
    fn event(&self, label: &str, value: Value) {
        let number = self.next.get() + 1;
        self.next.set(number);
        let mut file = File::create(self.root.join(format!("{number:03}-{label}.json"))).unwrap();
        file.write_all(value.to_string().as_bytes()).unwrap();
        file.sync_all().unwrap();
    }
    fn bound(&self) {
        fn bytes(path: &Path) -> u64 {
            fs::read_dir(path)
                .unwrap()
                .map(|entry| {
                    let entry = entry.unwrap();
                    let kind = entry.file_type().unwrap();
                    assert!(!kind.is_symlink(), "owned fixture cannot contain symlinks");
                    if kind.is_dir() {
                        bytes(&entry.path())
                    } else {
                        entry.metadata().unwrap().len()
                    }
                })
                .sum()
        }
        assert!(
            bytes(&self.root) <= 16 * MIB,
            "raw fixture/evidence exceeds16MiB; preserve it"
        );
    }
    fn finish(&mut self) {
        self.bound();
        // The registered root driver owns archive/readback/cleanup. Retain one
        // raw tree for it; ordinary workspace tests remove successful fixtures.
        self.event(
            "inventory",
            json!({"raw_cap_bytes":16*MIB,"files":inventory(&self.root)}),
        );
        self.bound();
        self.complete = true;
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if self.complete
            && !std::thread::panicking()
            && std::env::var_os("FABRIC_NATIVE_EVIDENCE").is_none()
        {
            fs::remove_dir_all(&self.root).unwrap();
        } else if self.complete && !std::thread::panicking() {
            eprintln!(
                "native lifecycle evidence retained at {}",
                self.root.display()
            );
        } else {
            eprintln!(
                "native lifecycle failure retained at {}",
                self.root.display()
            );
        }
    }
}
fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn inventory(root: &Path) -> BTreeMap<String, (u64, String)> {
    fn visit(root: &Path, path: &Path, out: &mut BTreeMap<String, (u64, String)>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            assert!(!kind.is_symlink());
            if kind.is_dir() {
                visit(root, &entry.path(), out);
            } else {
                assert!(kind.is_file());
                let bytes = fs::read(entry.path()).unwrap();
                out.insert(
                    entry
                        .path()
                        .strip_prefix(root)
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_owned(),
                    (bytes.len() as u64, hex(&bytes)),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

struct Running {
    handle: axum_server::Handle<std::net::SocketAddr>,
    thread: Option<std::thread::JoinHandle<std::io::Result<()>>>,
    addr: SocketAddr,
}
impl Running {
    fn stop(mut self) {
        self.handle.graceful_shutdown(Some(Duration::from_secs(5)));
        self.join(true);
    }
    fn join(&mut self, strict: bool) {
        if let Some(thread) = self.thread.take() {
            let until = Instant::now() + Duration::from_secs(10);
            while !thread.is_finished() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            if thread.is_finished() {
                let result = thread.join();
                if strict {
                    result.unwrap().unwrap();
                }
            } else if strict {
                self.handle.shutdown();
                self.thread = Some(thread);
                panic!("server shutdown timeout; root containment must reap thread");
            } else {
                eprintln!("server shutdown did not complete; root containment must reap thread");
            }
        }
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.handle.shutdown();
            self.join(false);
        }
    }
}
fn config(root: &Path, plan: Plan) -> Config {
    Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        tls_cert: root.join("server.pem"),
        tls_key: root.join("server.key"),
        state_dir: root.join("server-state"),
        admin_token_file: root.join("admin-token"),
        console_dir: None,
        access_origin: None,
        access_rp_id: None,
        journal_bytes: 2 * MIB,
        journal_file_bytes: 64 * 1024,
        retention_s: 86400,
        retention_bytes: 4 * MIB,
        query_plan: plan,
        seal_workers: 1,
    }
}
fn start(cfg: Config) -> Running {
    check_time();
    let handle = axum_server::Handle::new();
    let serving = handle.clone();
    let thread = std::thread::spawn(move || {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
            .block_on(fabric_server::serve(cfg, CommitMode::GROUPED, serving))
    });
    let mut running = Running {
        handle,
        thread: Some(thread),
        addr: "127.0.0.1:0".parse().unwrap(),
    };
    let address = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(10), running.handle.listening())
                .await
                .ok()
                .flatten()
        });
    running.addr = address.expect("server did not listen; RAII owner stops startup");
    running
}
fn node_config(root: &Path, addr: SocketAddr) -> NodeConfig {
    NodeConfig {
        spool: root.join("spool"),
        logs: vec![root.join("input.log")],
        interval_s: 3600,
        spool_bytes: 2 * MIB,
        server: Some(ServerTarget {
            url: format!("https://{addr}"),
            ca: root.join("ca.pem"),
            token_file: root.join("node-token"),
        }),
        traces_listen: None,
        max_output_bytes_per_s: None,
    }
}
type Identity = (Vec<u8>, u64, u64);
type Producer = BTreeMap<Identity, Vec<u8>>;
fn identity(raw: &[u8]) -> Identity {
    let batch = Batch::decode(raw).unwrap();
    (batch.node_id, batch.generation, batch.sequence)
}
fn capture(s: &Scratch, producer: &mut Producer) {
    let snapshot = FrameLog::inspect(
        &s.root.join("spool"),
        2 * MIB - 4096,
        fabric_frame::envelope::MAX_BATCH,
        |bytes, _| {
            let key = identity(bytes);
            if let Some(old) = producer.get(&key) {
                assert_eq!(old, bytes);
            } else {
                producer.insert(key, bytes.to_vec());
            }
            Ok(())
        },
    )
    .unwrap();
    assert!(
        !snapshot.recovery_required && !snapshot.interrupted_append,
        "producer snapshot must be complete"
    );
    s.event("producer-before-send",json!({"records":producer.iter().map(|(key,bytes)|json!({"node_id":key.0,"generation":key.1,
        "sequence":key.2,"sha256":hex(bytes),"bytes_hex":bytes.iter().map(|b|format!("{b:02x}")).collect::<String>()})).collect::<Vec<_>>()}));
    s.bound();
}
fn send(s: &Scratch, node: &mut Spindle, producer: &mut Producer) {
    check_time();
    capture(s, producer);
    let report=node.deliver(Instant::now()+Duration::from_secs(15),|attempt|s.event("delivery-attempt",json!({
        "sequence":attempt.sequence,"sha256":attempt.sha256,"outcome":format!("{:?}",attempt.outcome),"elapsed_us":attempt.elapsed_us}))).unwrap();
    s.event("delivery-result",json!({"sent":report.sent,"acked":report.acked_through,"caught_up":report.caught_up,"error":report.error}));
    assert!(report.caught_up);
    assert!(report.error.is_none());
    assert_eq!(report.acked_through, producer.len() as u64);
}
fn recovered(s: &Scratch, producer: &Producer) -> Vec<Entry> {
    let cfg = config(&s.root, Plan::Scan);
    let mut entries = Vec::new();
    Store::replay(&cfg.state_dir, cfg.journal_bytes, |entry| {
        entries.push(entry.clone());
        Ok(())
    })
    .unwrap();
    let mut actual = Producer::new();
    for entry in &entries {
        assert_eq!(entry.label, "native-node");
        assert!(
            actual
                .insert(identity(&entry.batch), entry.batch.clone())
                .is_none()
        );
    }
    s.event(
        "custody-bijection",
        json!({"producer":producer.len(),"recovered":entries.len(),"same":actual==*producer}),
    );
    assert_eq!(
        &actual, producer,
        "replay must match producer BEFORE it supplies oracle timestamps"
    );
    assert_eq!(
        producer
            .keys()
            .map(|key| (&key.0, key.1))
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        1,
        "restart must retain one producer strand"
    );
    assert_eq!(
        producer.keys().map(|key| key.2).collect::<Vec<_>>(),
        (1..=producer.len() as u64).collect::<Vec<_>>()
    );
    let mut missing = actual.clone();
    missing.pop_first();
    assert_ne!(&missing, producer, "missing recovered negative control");
    entries
}
fn b64(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::new();
    for chunk in bytes.chunks(3) {
        let value = chunk.iter().enumerate().fold(0_u32, |sum, (index, byte)| {
            sum | (u32::from(*byte) << (16 - 8 * index))
        });
        for index in 0..4 {
            output.push(if index <= chunk.len() {
                TABLE[((value >> (18 - 6 * index)) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    output
}
fn query(s: &Scratch, addr: SocketAddr, body: &Value) -> Value {
    check_time();
    let agent = sender::agent(&s.root.join("ca.pem")).unwrap();
    let mut response = agent
        .post(&format!("https://{addr}/v1/admin/query"))
        .header("authorization", &format!("Bearer {ADMIN}"))
        .header("content-type", "application/json")
        .send(body.to_string())
        .unwrap();
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .with_config()
        .limit(256 * 1024)
        .read_to_string()
        .unwrap();
    assert_eq!(status, 200, "{text}");
    let answer: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(answer["complete"], true);
    answer
}
fn pages(s: &Scratch, addr: SocketAddr, body: &Value) -> Vec<Value> {
    let mut request = body.clone();
    let mut pages = Vec::new();
    for _ in 0..64 {
        let page = query(s, addr, &request);
        let next = page["next_page"].clone();
        pages.push(page);
        if next.is_null() {
            return pages;
        }
        request["page"] = next;
    }
    panic!("page chain exceeded64 pages")
}
fn oracle(
    s: &Scratch,
    label: &str,
    records: &[Entry],
    query: &Value,
    pages: &[Value],
    expect_pass: bool,
) {
    let record_path = s.root.join(format!("{label}-records.jsonl"));
    let raw=records.iter().map(|entry|json!({"label":entry.label,"received_ns":entry.received_unix_nano,"bytes":b64(&entry.batch)}).to_string()+"\n").collect::<String>();
    fs::write(&record_path, raw).unwrap();
    let query_path = s.root.join(format!("{label}-query.json"));
    let answer_path = s.root.join(format!("{label}-answer.json"));
    fs::write(&query_path, query.to_string()).unwrap();
    fs::write(&answer_path, serde_json::to_vec(pages).unwrap()).unwrap();
    let mut command = Command::new("python3");
    command
        .arg("-B")
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/qualification/query_oracle.py"),
        )
        .arg("--records")
        .arg(record_path)
        .arg("--query")
        .arg(query_path)
        .arg("--answer")
        .arg(answer_path);
    // The oracle CLI exits nonzero for rejected mutations; capture that output
    // rather than treating its intended verdict as a subprocess failure.
    check_time();
    let mut child = command
        .stdout(File::create(s.root.join(format!("{label}.stdout"))).unwrap())
        .stderr(File::create(s.root.join(format!("{label}.stderr"))).unwrap())
        .spawn()
        .unwrap();
    let until = (*DEADLINE.get().unwrap()).min(Instant::now() + Duration::from_secs(15));
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= until {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("oracle deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let verdict: Value =
        serde_json::from_slice(&fs::read(s.root.join(format!("{label}.stdout"))).unwrap()).unwrap();
    s.event(
        "oracle-verdict",
        json!({"label":label,"exit":status.code(),"verdict":verdict,"expected_pass":expect_pass}),
    );
    assert_eq!(verdict["passed"], expect_pass);
    if expect_pass {
        assert!(status.success());
    }
    s.bound();
}
fn grade_stage(s: &Scratch, producer: &Producer, label: &str) {
    let queries = [
        json!({"kind":"logs","from_ns":0,"to_ns":u64::MAX/2,"limit":7}),
        json!({"kind":"logs","contains":"λ🦀","from_ns":0,"to_ns":u64::MAX/2,"limit":3}),
        json!({"kind":"metrics","name":"system.network.receive.bytes","from_ns":0,"to_ns":u64::MAX/2,"limit":2}),
        json!({"kind":"rate","name":"system.network.receive.bytes","from_ns":0,"to_ns":u64::MAX/2}),
        json!({"kind":"spans","from_ns":0,"to_ns":u64::MAX/2,"limit":1}),
    ];
    let mut reference = None;
    for plan in [Plan::Scan, Plan::Walk] {
        let server = start(config(&s.root, plan));
        let answers = queries
            .iter()
            .map(|q| pages(s, server.addr, q))
            .collect::<Vec<_>>();
        server.stop();
        assert!(
            !answers[0][0]["rows"].as_array().unwrap().is_empty(),
            "actual collected log must be query-visible"
        );
        assert!(
            !answers[2][0]["rows"].as_array().unwrap().is_empty(),
            "actual host metric must be query-visible"
        );
        assert_eq!(
            answers[4]
                .iter()
                .map(|page| page["rows"].as_array().unwrap().len())
                .sum::<usize>(),
            1,
            "public trace commit must be query-visible"
        );
        let records = recovered(s, producer);
        for (index, (query, answer)) in queries.iter().zip(&answers).enumerate() {
            oracle(
                s,
                &format!("{label}-{plan:?}-{index}"),
                &records,
                query,
                answer,
                true,
            );
        }
        if let Some(previous) = &reference {
            assert_eq!(&answers, previous, "Scan/Walk whole answers differ");
        } else {
            reference = Some(answers.clone());
        }
        let mut missing = answers[0].clone();
        missing[0]["rows"].as_array_mut().unwrap().pop();
        oracle(
            s,
            &format!("{label}-{plan:?}-missing"),
            &records,
            &queries[0],
            &missing,
            false,
        );
        let mut duplicate = answers[0].clone();
        let row = duplicate[0]["rows"][0].clone();
        duplicate[0]["rows"].as_array_mut().unwrap().push(row);
        oracle(
            s,
            &format!("{label}-{plan:?}-duplicate"),
            &records,
            &queries[0],
            &duplicate,
            false,
        );
    }
}

#[test]
fn actual_collection_tls_restart_publication_preserve_exact_history() {
    let mut scratch = Scratch::new();
    let s = &scratch;
    make_certs(&s.root);
    write_host(&s.root);
    fs::write(s.root.join("admin-token"), ADMIN).unwrap();
    let mut source = vec![b'x'; (3 * MIB) as usize];
    source.extend_from_slice("\nnormal-λ🦀\n".as_bytes());
    fs::write(s.root.join("input.log"), &source).unwrap();
    assert!(source.len() as u64 <= 3 * MIB + 128 * 1024);
    let mut control = Control::open(&config(&s.root, Plan::Scan).state_dir).unwrap();
    let (_, token) = control
        .enroll(
            "native-node",
            DesiredConfig {
                logs: vec![s.root.join("input.log").to_str().unwrap().into()],
                metric_interval_s: 3600,
            },
        )
        .unwrap();
    fs::write(s.root.join("node-token"), token).unwrap();
    drop(control);
    s.bound();
    let mut server = start(config(&s.root, Plan::Scan));
    let mut node =
        Spindle::open_with_paths(node_config(&s.root, server.addr), host_paths(&s.root)).unwrap();
    let mut producer = Producer::new();
    for pass in 1..=4 {
        check_time();
        let cycle = node
            .collect_logs()
            .unwrap()
            .expect("skip pass must commit progress");
        s.event("skip-pass",json!({"pass":pass,"sequence":cycle.batch_sequence,"logs":cycle.log_records,"metrics":cycle.metric_points,"gaps":cycle.gaps}));
        assert_eq!(cycle.batch_sequence, pass);
        if pass == 1 {
            assert_eq!((cycle.log_records, cycle.gaps), (0, 1));
        }
        if pass == 4 {
            assert_eq!(cycle.log_records, 1);
        }
        send(s, &mut node, &mut producer);
        let batch = Batch::decode(producer.values().last().unwrap().as_slice()).unwrap();
        let cursor = batch
            .cursors
            .iter()
            .find(|cursor| cursor.path == s.root.join("input.log").to_str().unwrap())
            .unwrap();
        assert_eq!(
            cursor.offset,
            if pass < 4 {
                pass * MIB
            } else {
                source.len() as u64
            }
        );
        assert_eq!(cursor.skipping_oversize, pass < 4);
        if pass == 2 {
            drop(node);
            server.stop();
            let records = recovered(s, &producer);
            assert_eq!(records.len(), 2);
            server = start(config(&s.root, Plan::Scan));
            node = Spindle::open_with_paths(node_config(&s.root, server.addr), host_paths(&s.root))
                .unwrap();
            assert_eq!(node.acked_through(), 2);
            let duplicate = producer.values().last().unwrap();
            let sender =
                Sender::new(node_config(&s.root, server.addr).server.as_ref().unwrap()).unwrap();
            let response = sender.send(duplicate);
            s.event(
                "lost-ack-replay",
                json!({"sha256":hex(duplicate),"response":format!("{response:?}")}),
            );
            assert!(matches!(response, Delivery::Ack(2)));
        }
    }
    use opentelemetry_proto::tonic::{
        collector::trace::v1::ExportTraceServiceRequest,
        trace::v1::{ResourceSpans, ScopeSpans, Span},
    };
    let trace = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans: vec![Span {
                    trace_id: vec![7; 16],
                    span_id: vec![8; 8],
                    name: "native-public-commit".into(),
                    start_time_unix_nano: 123,
                    end_time_unix_nano: 456,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec();
    node.commit_traces(&[trace.as_slice()]).unwrap();
    send(s, &mut node, &mut producer);
    drop(node);
    server.stop();
    assert!(
        fabric_server::segment::list(&config(&s.root, Plan::Scan).state_dir)
            .unwrap()
            .is_empty()
    );
    grade_stage(s, &producer, "tail");
    server = start(config(&s.root, Plan::Scan));
    node =
        Spindle::open_with_paths(node_config(&s.root, server.addr), host_paths(&s.root)).unwrap();
    let mut normal = OpenOptions::new()
        .append(true)
        .open(s.root.join("input.log"))
        .unwrap();
    let mut expected_bodies = vec!["normal-λ🦀".to_owned()];
    for index in 0..40 {
        let prefix = format!("native-{index:02}-λ🦀-");
        let line = prefix.clone() + &"R".repeat(2048 - prefix.len()) + "\n";
        assert_eq!(line.len(), 2049);
        expected_bodies.push(line.trim_end_matches('\n').to_owned());
        normal.write_all(line.as_bytes()).unwrap();
    }
    drop(normal);
    let cycle = node.collect_logs().unwrap().unwrap();
    assert_eq!(cycle.log_records, 40);
    send(s, &mut node, &mut producer);
    // Rotation is checked before the next commit, so use one real metrics
    // collection after the bounded >64KiB log Batch; never invent journal labels.
    node.collect_once().unwrap();
    send(s, &mut node, &mut producer);
    let state = config(&s.root, Plan::Scan).state_dir;
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        check_time();
        let segments = fabric_server::segment::list(&state).unwrap();
        let sealed = fs::read_dir(state.join("journal"))
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("sealed-")
            })
            .count();
        if !segments.is_empty() && sealed == 0 {
            break;
        }
        assert!(
            Instant::now() < until,
            "published Segment/reclaimed journal not observed"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(node);
    server.stop();
    recovered(s, &producer);
    grade_stage(s, &producer, "published");
    grade_stage(s, &producer, "restarted-published");
    let mut bodies = Vec::new();
    let mut gaps = Vec::new();
    for raw in producer.values() {
        let batch = Batch::decode(raw.as_slice()).unwrap();
        gaps.extend(batch.collection_gaps);
        if !batch.logs.is_empty() {
            let logs =
                opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest::decode(
                    batch.logs.as_slice(),
                )
                .unwrap();
            for resource in logs.resource_logs {
                for scope in resource.scope_logs {
                    for record in scope.log_records {
                        if let Some(
                            opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(
                                body,
                            ),
                        ) = record.body.and_then(|value| value.value)
                        {
                            bodies.push(body);
                        }
                    }
                }
            }
        }
    }
    assert_eq!(
        bodies, expected_bodies,
        "producer decoded bodies must equal independently constructed source lines"
    );
    assert_eq!(gaps.len(), 1);
    assert!(gaps[0].starts_with("oversize log line skipped:"));
    s.event("final",json!({"producer_batches":producer.len(),"source_log_rows":41,"trace_path":"public commit_traces; not OTLP listener",
        "query_chains":30,"negative_query_controls":12,"segments":fabric_server::segment::list(&state).unwrap().len(),"exact":true}));
    scratch.finish();
}
