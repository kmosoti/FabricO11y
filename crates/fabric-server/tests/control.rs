//! Central control end to end: admin API, node polling, pause, revoke,
//! validation before activation and last-valid configuration offline.

use fabric_o11y::spindle::host::Paths;
use fabric_o11y::spindle::runtime::{Config as NodeConfig, Spindle};
use fabric_o11y::spindle::sender::{ServerTarget, agent};
use fabric_server::config::Config;
use fabric_server::store::CommitMode;
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
            .join(format!("control-test-{}-{id}", std::process::id()));
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

fn start(root: &Path) -> Running {
    let config = Config {
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
    };
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

struct Admin {
    agent: ureq::Agent,
    base: String,
    auth: String,
}

impl Admin {
    fn new(root: &Path, addr: SocketAddr, token: &str) -> Self {
        Self {
            agent: agent(&root.join("ca.pem")).unwrap(),
            base: format!("https://{addr}/v1/admin/nodes"),
            auth: format!("Bearer {token}"),
        }
    }
    fn call(&self, method: &str, tail: &str, body: Option<Value>) -> (u16, Value) {
        let url = format!("{}{tail}", self.base);
        let result = match (method, body) {
            ("GET", _) => self
                .agent
                .get(&url)
                .header("authorization", &self.auth)
                .call(),
            ("POST", Some(b)) => self
                .agent
                .post(&url)
                .header("authorization", &self.auth)
                .header("content-type", "application/json")
                .send(b.to_string()),
            ("POST", None) => self
                .agent
                .post(&url)
                .header("authorization", &self.auth)
                .send_empty(),
            ("PUT", Some(b)) => self
                .agent
                .put(&url)
                .header("authorization", &self.auth)
                .header("content-type", "application/json")
                .send(b.to_string()),
            _ => unreachable!(),
        };
        let mut response = result.unwrap();
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
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

fn gaps(spool: &Path) -> Vec<String> {
    let mut out = Vec::new();
    fabric_o11y::spindle::spool::Spool::inspect(spool, 16 * 1024 * 1024, |b| {
        out.extend(b.collection_gaps);
        Ok(())
    })
    .unwrap();
    out
}

#[test]
fn admin_api_configures_pauses_and_revokes_a_polling_node() {
    let scratch = Scratch::new();
    make_certs(&scratch.0);
    write_host(&scratch.0);
    let admin_token: String = Sha256::digest(b"admin")
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    fs::write(scratch.path("admin-token"), format!("{admin_token}\n")).unwrap();
    fs::write(scratch.path("app.log"), "one\n").unwrap();
    let server = start(&scratch.0);
    let admin = Admin::new(&scratch.0, server.addr, &admin_token);

    // The admin surface refuses a missing or wrong token.
    let nobody = Admin::new(
        &scratch.0,
        server.addr,
        "wrong-token-wrong-token-wrong-token",
    );
    assert_eq!(nobody.call("GET", "", None).0, 401);
    assert_eq!(
        admin.call("POST", "", Some(json!({"name": "bad name!"}))).0,
        400
    );
    assert_eq!(
        admin
            .call(
                "POST",
                "",
                Some(json!({"name": "n1", "metric_interval_s": 0}))
            )
            .0,
        400
    );
    let (status, enrolled) = admin.call("POST", "", Some(json!({"name": "n1"})));
    assert_eq!(status, 201);
    let token = enrolled["token"].as_str().unwrap().to_owned();
    assert_eq!(admin.call("POST", "", Some(json!({"name": "n1"}))).0, 409);
    // A node token never authorizes admin routes.
    assert_eq!(
        Admin::new(&scratch.0, server.addr, &token)
            .call("GET", "", None)
            .0,
        401
    );
    fs::write(scratch.path("token"), format!("{token}\n")).unwrap();

    let base = NodeConfig {
        spool: scratch.path("spool"),
        logs: vec![],
        interval_s: 15,
        spool_bytes: 16 * 1024 * 1024,
        server: Some(ServerTarget {
            url: format!("https://{}", server.addr),
            ca: scratch.path("ca.pem"),
            token_file: scratch.path("token"),
        }),
    };
    let mut node = Spindle::open_with_paths(base.clone(), host_paths(&scratch.0)).unwrap();
    assert!(node.poll_config().unwrap().changed);
    assert_eq!(node.applied_revision(), 1);
    assert!(
        !node.poll_config().unwrap().changed,
        "an unchanged revision is a 304"
    );

    // Live reconfiguration: the new log is read after the next poll.
    let log = scratch.path("app.log").to_string_lossy().into_owned();
    let (status, changed) = admin.call(
        "PUT",
        "/n1/config",
        Some(json!({"logs": [log], "metric_interval_s": 2})),
    );
    assert_eq!((status, changed["revision"].as_u64()), (200, Some(2)));
    assert!(node.poll_config().unwrap().changed);
    assert_eq!((node.applied_revision(), node.interval_s()), (2, 2));
    assert_eq!(node.collect_once().unwrap().log_records, 1);
    // The confirming poll (a 304) reports the applied revision.
    assert!(!node.poll_config().unwrap().changed);
    let (_, list) = admin.call("GET", "", None);
    assert_eq!(list["nodes"][0]["applied_revision"], 2);
    assert_eq!(list["nodes"][0]["desired_revision"], 2);

    // The server refuses a configuration outside the shared profile.
    let long = format!("/{}", "x".repeat(240));
    assert_eq!(
        admin
            .call(
                "PUT",
                "/n1/config",
                Some(json!({"logs": [long], "metric_interval_s": 2}))
            )
            .0,
        400
    );
    assert_eq!(
        admin
            .call(
                "PUT",
                "/n1/config",
                Some(json!({"logs": ["relative.log"], "metric_interval_s": 2}))
            )
            .0,
        400
    );

    // Pause stops collection; resume reports the paused interval once.
    assert_eq!(admin.call("POST", "/n1/pause", None).0, 200);
    assert!(node.poll_config().unwrap().changed && node.paused());
    fs::write(scratch.path("app.log"), "one\ntwo\n").unwrap();
    assert!(node.collect_logs().unwrap().is_none());
    assert_eq!(admin.call("POST", "/n1/resume", None).0, 200);
    assert!(node.poll_config().unwrap().changed && !node.paused());
    let resumed = node.collect_logs().unwrap().unwrap();
    assert_eq!((resumed.log_records, resumed.gaps), (1, 1));
    let notices = gaps(&scratch.path("spool"))
        .into_iter()
        .filter(|g| g.starts_with("coverage unknown since "))
        .count();
    assert_eq!(notices, 1);
    let report = node
        .deliver(Instant::now() + Duration::from_secs(20), |_| {})
        .unwrap();
    assert!(report.caught_up, "{report:?}");

    // Revoke: intake and polling refuse the token; the node keeps its spool and config.
    assert_eq!(admin.call("POST", "/n1/revoke", None).0, 200);
    assert!(node.poll_config().unwrap().error.is_some());
    node.collect_once().unwrap();
    let report = node
        .deliver(Instant::now() + Duration::from_secs(5), |_| {})
        .unwrap();
    assert!(report.error.unwrap().starts_with("HTTP 401"));
    assert_eq!(node.interval_s(), 2);
    assert_eq!(
        admin.call("POST", "/n1/resume", None).0,
        400,
        "a revoked node stays revoked"
    );
    drop(node);
    server.stop();

    // Server restart keeps the inventory; a node restarted while the server
    // is down runs its last applied configuration.
    let node = Spindle::open_with_paths(base.clone(), host_paths(&scratch.0)).unwrap();
    assert_eq!((node.applied_revision(), node.interval_s()), (4, 2));
    drop(node);
    let server = start(&scratch.0);
    let admin = Admin::new(&scratch.0, server.addr, &admin_token);
    let (_, list) = admin.call("GET", "", None);
    assert_eq!(list["nodes"][0]["status"], "revoked");
    server.stop();

    // A stored configuration that fails the node's own validation is not
    // activated on restart; the node runs its local base instead.
    fs::write(
        scratch.path("spool").join("applied-config.json"),
        r#"{"revision":9,"paused":false,"logs":["relative.log"],"metric_interval_s":2}"#,
    )
    .unwrap();
    let node = Spindle::open_with_paths(base, host_paths(&scratch.0)).unwrap();
    assert_eq!((node.applied_revision(), node.interval_s()), (0, 15));
}
