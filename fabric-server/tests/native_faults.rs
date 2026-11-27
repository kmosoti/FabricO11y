//! Delayed real-engine writes and an actual gateway process death.
//! Run only against a fresh, explicitly owned Greptime instance.
use fabric::canonical::{self, ForwardRecord, Request, Response};
use fabric_server::{Config, NodeGrant, ledger::Ledger};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
const RAW: &str = r#" {"version":2,"registry":"fabrico11y/2","identity":{"producer":"fault","generation":"a","sequence":18446744073709551615},"timestamp_ns":18446744073709551615,"resource":"service","data":{"kind":"metric","name":"finite","unit":"1","value":-0.0,"metric_type":"gauge","reset":false}} "#;
fn record(id: u64, raw: &str) -> ForwardRecord {
    ForwardRecord {
        source_id: id,
        raw: raw.into(),
    }
}
struct Gateway {
    child: Child,
    endpoint: String,
}
impl Gateway {
    fn start(config: &Config, path: &Path) -> Self {
        fs::write(path, serde_json::to_vec(config).unwrap()).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_fabric-server"))
            .args(["serve", path.to_str().unwrap()])
            .stdout(Stdio::piped())
            .stderr(fs::File::create(path.with_extension("stderr")).unwrap())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(stdout).read_line(&mut line);
            let _ = tx.send(line);
        });
        let line = rx.recv_timeout(Duration::from_secs(10)).unwrap_or_default();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap_or_else(|_| {
            let _ = child.kill();
            panic!(
                "gateway startup: {}",
                fs::read_to_string(path.with_extension("stderr")).unwrap_or_default()
            )
        });
        let endpoint = value["address"].as_str().unwrap().to_owned();
        Self { child, endpoint }
    }
    fn kill(&mut self) {
        let _ = self.child.kill();
        self.child.wait().unwrap();
    }
    fn call(&self, credential: &str, request: Request) -> Response {
        canonical::exchange(
            &self.endpoint,
            credential,
            &request,
            Instant::now() + Duration::from_secs(9),
        )
        .unwrap()
    }
}
impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
struct Proxy {
    endpoint: String,
    stop: Arc<AtomicBool>,
    resume: Option<mpsc::Sender<()>>,
    captured: mpsc::Receiver<()>,
    late: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Proxy {
    fn start(engine: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let exit = stop.clone();
        let first = Arc::new(AtomicBool::new(true));
        let late = Arc::new(AtomicBool::new(false));
        let done = late.clone();
        let (capture_tx, captured) = mpsc::channel();
        let (resume, resume_rx) = mpsc::channel();
        let resume_rx = Arc::new(Mutex::new(Some(resume_rx)));
        let upstream = format!("{}/v1/sql", engine.trim_end_matches('/'));
        let worker = thread::spawn(move || {
            let client = reqwest::blocking::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap();
            let mut children = Vec::new();
            while !exit.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let client = client.clone();
                        let url = upstream.clone();
                        let first = first.clone();
                        let signal = capture_tx.clone();
                        let release = resume_rx.clone();
                        let done = done.clone();
                        children.push(thread::spawn(move||{let _=(||->Result<(),String>{
      let body=http_body(&mut stream)?;
      let insert=body.windows(b"INSERT+INTO+fabric_canonical_v1".len()).any(|w|w==b"INSERT+INTO+fabric_canonical_v1");
      let delayed=insert&&first.compare_exchange(true,false,Ordering::SeqCst,Ordering::SeqCst).is_ok();
      if delayed{signal.send(()).map_err(err)?;release.lock().map_err(err)?.take().ok_or("missing release")?.recv_timeout(Duration::from_secs(15)).map_err(err)?;}
      let response=client.post(url).header("Content-Type","application/x-www-form-urlencoded").body(body).send().map_err(err)?;let status=response.status().as_u16();let mut bytes=Vec::new();response.take(1024*1024+1).read_to_end(&mut bytes).map_err(err)?;
      if bytes.len()>1024*1024{return Err("proxy reply limit".into())}
      if delayed{done.store(true,Ordering::SeqCst);}
      write!(stream,"HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",bytes.len()).map_err(err)?;stream.write_all(&bytes).map_err(err)?;Ok(())})();}));
                        children.retain(|t| !t.is_finished());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1))
                    }
                    Err(_) => break,
                }
            }
            for child in children {
                let _ = child.join();
            }
        });
        Self {
            endpoint,
            stop,
            resume: Some(resume),
            captured,
            late,
            worker: Some(worker),
        }
    }
    fn release(&mut self) {
        if let Some(tx) = self.resume.take() {
            let _ = tx.send(());
        }
    }
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.release();
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.worker.take() {
            t.join().unwrap();
        }
    }
}
fn http_body(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(err)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(err)?;
    let mut bytes = Vec::new();
    let mut scratch = [0; 4096];
    let end = loop {
        if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if bytes.len() > 16384 {
            return Err("proxy header limit".into());
        }
        let n = stream.read(&mut scratch).map_err(err)?;
        if n == 0 {
            return Err("proxy EOF".into());
        }
        bytes.extend_from_slice(&scratch[..n]);
    };
    let header = std::str::from_utf8(&bytes[..end]).map_err(err)?;
    let length: usize = header
        .lines()
        .find_map(|l| {
            l.split_once(':')
                .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .map(|(_, v)| v.trim())
        })
        .ok_or("proxy length")?
        .parse()
        .map_err(err)?;
    if length > 1024 * 1024 {
        return Err("proxy request limit".into());
    }
    while bytes.len() < end + length {
        let n = stream.read(&mut scratch).map_err(err)?;
        if n == 0 {
            return Err("proxy body EOF".into());
        }
        bytes.extend_from_slice(&scratch[..n]);
    }
    Ok(bytes[end..end + length].to_vec())
}
#[test]
#[ignore = "requires fresh explicitly owned FABRIC_GREPTIME_ENDPOINT"]
fn unknown_inflight_write_and_gateway_death_preserve_pending_binding() {
    let engine = std::env::var("FABRIC_GREPTIME_ENDPOINT").expect("fresh owned engine endpoint");
    let mut proxy = Proxy::start(&engine);
    let root = tempfile::tempdir().unwrap();
    let reader = "native-test-reader-private-key";
    let node = "native-test-node-private-key";
    for (name, value) in [("reader", reader), ("node", node)] {
        let path = root.path().join(name);
        fs::write(&path, value).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let raw2 = RAW.replace(
        "18446744073709551615},\"timestamp_ns\"",
        "2},\"timestamp_ns\"",
    );
    let mut config = Config {
        listen: "127.0.0.1:0".into(),
        storage: root.path().join("control"),
        projection: root.path().join("projection"),
        quota_bytes: 8 << 20,
        projection_quota_bytes: 8 << 20,
        reader_credential_file: root.path().join("reader"),
        nodes: vec![NodeGrant {
            origin: "owned-fault-origin".into(),
            credential_file: root.path().join("node"),
        }],
        node_quota_bytes: [("owned-fault-origin".into(), (RAW.len() + raw2.len()) as u64)].into(),
        engine_endpoint: Some(proxy.endpoint.clone()),
        engine_auth_token_file: None,
        allow_unauthenticated_loopback_engine: true,
        max_connections: 16,
    };
    let mut gateway = Gateway::start(&config, &root.path().join("server.json"));
    let address = gateway.endpoint.clone();
    let write = thread::spawn(move || {
        canonical::exchange(
            &address,
            node,
            &Request::Ingest {
                origin: "owned-fault-origin".into(),
                records: vec![record(1, RAW)],
            },
            Instant::now() + Duration::from_secs(9),
        )
        .unwrap()
    });
    proxy.captured.recv_timeout(Duration::from_secs(5)).unwrap();
    let started = Instant::now();
    let progress = gateway.call(reader, Request::Progress);
    assert!(
        matches!(progress,Response::Progress{ref frontiers,..} if frontiers.get("owned-fault-origin").copied().unwrap_or(0)==0)
    );
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "catalog waits for stalled engine write"
    );
    assert!(
        matches!(write.join().unwrap(), Response::Error { .. }),
        "native write deadline must leave outcome unknown"
    );
    // This retry differs by one whitespace byte. It first reconciles the immutable
    // pending original, then refuses the conflict. The delayed first request has
    // not reached Greptime yet.
    assert!(matches!(
        gateway.call(
            node,
            Request::Ingest {
                origin: "owned-fault-origin".into(),
                records: vec![record(1, RAW.trim())]
            }
        ),
        Response::Error { .. }
    ));
    proxy.release();
    let deadline = Instant::now() + Duration::from_secs(6);
    while !proxy.late.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    let Response::Rows { rows } = gateway.call(
        node,
        Request::Read {
            origin: "owned-fault-origin".into(),
            after: 0,
            through: 1,
            limit: 1,
        },
    ) else {
        panic!("retained original")
    };
    assert_eq!(rows[0].raw, RAW);
    let canonical_id = rows[0].canonical_id;
    // Create an engine-only durable pending tail through the same actual ingest
    // path, then SIGKILL while a new proxy holds its first write. On restart the
    // reservation, original bytes and ID must survive without a Fabric receipt.
    gateway.kill();
    let old_control = root.path().join("old-control.sqlite");
    fs::copy(config.storage.join("canonical.sqlite"), &old_control).unwrap();
    drop(proxy);
    let mut proxy = Proxy::start(&engine);
    config.engine_endpoint = Some(proxy.endpoint.clone());
    let mut gateway = Gateway::start(&config, &root.path().join("server2.json"));
    let second_raw = raw2.clone();
    let address = gateway.endpoint.clone();
    let writer = thread::spawn(move || {
        canonical::exchange(
            &address,
            node,
            &Request::Ingest {
                origin: "owned-fault-origin".into(),
                records: vec![record(2, &second_raw)],
            },
            Instant::now() + Duration::from_secs(9),
        )
    });
    proxy.captured.recv_timeout(Duration::from_secs(5)).unwrap();
    let started = Instant::now();
    let Response::Rows { rows } = gateway.call(
        node,
        Request::Read {
            origin: "owned-fault-origin".into(),
            after: 0,
            through: 1,
            limit: 1,
        },
    ) else {
        panic!("acknowledged read blocked by pending engine write")
    };
    assert_eq!(rows[0].raw, RAW);
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "acknowledged read owns the engine writer lock"
    );
    gateway.kill();
    let _ = writer.join().unwrap();
    proxy.release();
    let deadline = Instant::now() + Duration::from_secs(6);
    while !proxy.late.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    drop(proxy);
    config.engine_endpoint = Some(engine.clone());
    let current_control = root.path().join("current-control.sqlite");
    fs::rename(config.storage.join("canonical.sqlite"), &current_control).unwrap();
    fs::copy(&old_control, config.storage.join("canonical.sqlite")).unwrap();
    let error = match Ledger::open_with_engine_auth(
        &config.storage,
        config.quota_bytes,
        Some(&engine),
        None,
        true,
    ) {
        Ok(_) => panic!("old control accepted newer engine tail"),
        Err(error) => error,
    };
    assert!(error.contains("newer than its control history"), "{error}");
    fs::rename(
        config.storage.join("canonical.sqlite"),
        root.path().join("refused-control.sqlite"),
    )
    .unwrap();
    fs::rename(current_control, config.storage.join("canonical.sqlite")).unwrap();
    let gateway = Gateway::start(&config, &root.path().join("server3.json"));
    let Response::Progress { frontiers, .. } = gateway.call(reader, Request::Progress) else {
        panic!("recovered prefix")
    };
    assert_eq!(frontiers["owned-fault-origin"], 2);
    let Response::Rows { rows } = gateway.call(
        node,
        Request::Read {
            origin: "owned-fault-origin".into(),
            after: 0,
            through: 2,
            limit: 2,
        },
    ) else {
        panic!("recovered bytes")
    };
    assert_eq!(rows[0].raw, RAW);
    assert_eq!(rows[0].canonical_id, canonical_id);
    assert_eq!(rows[1].raw, raw2);
    assert_eq!(rows[1].canonical_id, canonical_id + 1);
    assert!(matches!(
        gateway.call(
            node,
            Request::Ingest {
                origin: "owned-fault-origin".into(),
                records: vec![record(1, RAW), record(2, &raw2)],
            },
        ),
        Response::Committed { through: 2, .. }
    ));
    let raw3 = raw2.replace("2},\"timestamp_ns\"", "3},\"timestamp_ns\"");
    assert!(matches!(
        gateway.call(
            node,
            Request::Ingest {
                origin: "owned-fault-origin".into(),
                records: vec![record(3, &raw3)],
            },
        ),
        Response::Error { error } if error.contains("node payload quota exhausted")
    ));
    drop(gateway);
    let ledger = Ledger::open_with_engine_auth(
        &config.storage,
        config.quota_bytes,
        Some(&engine),
        None,
        true,
    )
    .unwrap();
    assert_eq!(ledger.count().unwrap(), 2);
    let connection = rusqlite::Connection::open(config.storage.join("canonical.sqlite")).unwrap();
    let retained: u64 = connection
        .query_row(
            "SELECT SUM(length(raw)) FROM records WHERE published=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(retained, 0, "engine owns acknowledged raw history");
    assert!(
        Ledger::open(&config.storage, config.quota_bytes).is_err(),
        "mode cannot silently change after reclamation"
    );
}
