use fabric::{
    observation::{self, Data},
    run::RunContext,
    sdk::{Admission, RunClient},
    spindle::{self, Daemon},
};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
struct Service {
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    config: spindle::Config,
}
impl Service {
    fn start(root: &std::path::Path) -> Self {
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
        let config = spindle::Config {
            socket: root.join("socket"),
            storage: root.join("storage"),
            quota_bytes: 4 * 1024 * 1024,
            forwarding: None,
            max_connections: 8,
            client_timeout_ms: 500,
            files: vec![],
            journal: None,
            host: None,
        };
        let mut daemon = Daemon::open(config.clone()).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let exit = stop.clone();
        let pause = paused.clone();
        let worker = thread::spawn(move || {
            while !exit.load(Ordering::Relaxed) {
                if !pause.load(Ordering::Relaxed) {
                    daemon.tick().unwrap();
                }
                thread::sleep(Duration::from_millis(1));
            }
        });
        Self {
            stop,
            paused,
            thread: Some(worker),
            config,
        }
    }
    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.thread.take() {
            worker.join().unwrap();
        }
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.shutdown();
    }
}
fn record(client: &RunClient) -> Admission {
    let data = Data::Event {
        name: "fixture".into(),
        message: "shared run".into(),
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        assert!(Instant::now() < deadline, "admission deadline");
        let result = client.try_record(data.clone(), 1);
        match result {
            Admission::Contended | Admission::Full => {
                assert!(Instant::now() < deadline, "admission deadline: {result:?}");
                thread::sleep(Duration::from_millis(1));
            }
            _ => return result,
        }
    }
}
fn adapter_exchange(
    input: &mut std::process::ChildStdin,
    output: &mut impl BufRead,
    command: &serde_json::Value,
) -> serde_json::Value {
    writeln!(input, "{command}").unwrap();
    input.flush().unwrap();
    let mut line = String::new();
    assert_ne!(
        output.read_line(&mut line).unwrap(),
        0,
        "adapter closed without reply"
    );
    let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert!(reply.get("error").is_none(), "adapter error: {reply}");
    reply
}

#[test]
fn global_context_crosses_rust_and_jsonl_processes_and_renewal_is_quiescent() {
    let temp = tempfile::tempdir().unwrap();
    let mut service = Service::start(temp.path());
    let context = RunContext::new().unwrap();
    let config = fabric::sdk::Config::local(service.config.socket.clone(), "rust".into()).unwrap();
    let mut client = RunClient::new(config, context.clone()).unwrap();
    service.paused.store(true, Ordering::Relaxed);
    thread::sleep(Duration::from_millis(5));
    assert_eq!(record(&client), Admission::Accepted { sequence: 1 });
    assert!(client.renew().is_err());
    service.paused.store(false, Ordering::Relaxed);
    client
        .flush_local(Instant::now() + Duration::from_secs(2))
        .unwrap();
    assert!(
        client
            .flush_to_fabric(Instant::now() + Duration::from_secs(1))
            .unwrap_err()
            .contains("not configured")
    );
    client.renew().unwrap();
    assert_eq!(record(&client), Admission::Accepted { sequence: 1 });
    client
        .flush_local(Instant::now() + Duration::from_secs(2))
        .unwrap();
    let mut process = Command::new(env!("CARGO_BIN_EXE_fabric-adapter"))
        .arg(&service.config.socket)
        .arg("python-process")
        .arg("--run-context")
        .arg(serde_json::to_string(&context).unwrap())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = process.stdin.take().unwrap();
    let mut output = BufReader::new(process.stdout.take().unwrap());
    let command = serde_json::json!({"command":"record","data":{"kind":"tool_execution","tool":"python","run_id":context.run_id,"duration_ns":1,"status":"ok"},"timestamp_ns":2});
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        assert!(Instant::now() < deadline, "adapter admission deadline");
        let reply = adapter_exchange(&mut input, &mut output, &command);
        let admission: Admission = serde_json::from_value(reply["admission"].clone()).unwrap();
        match admission {
            Admission::Accepted { sequence } => {
                assert_eq!(sequence, 1);
                break;
            }
            Admission::Contended | Admission::Full => thread::sleep(Duration::from_millis(1)),
            other => panic!("adapter admission refused: {other:?}"),
        }
    }
    let flushed = adapter_exchange(
        &mut input,
        &mut output,
        &serde_json::json!({"command":"flush","timeout_ms":2000}),
    );
    assert!(
        flushed["receipt"]["id"].as_u64().is_some(),
        "missing receipt: {flushed}"
    );
    assert_eq!(flushed["receipt"]["identity"]["sequence"], 1);
    let returned = adapter_exchange(
        &mut input,
        &mut output,
        &serde_json::json!({"command":"context"}),
    );
    assert_eq!(returned["context"], serde_json::to_value(&context).unwrap());
    drop(input);
    let mut trailing = String::new();
    output.read_to_string(&mut trailing).unwrap();
    let eof: serde_json::Value = serde_json::from_str(&trailing).unwrap();
    assert_eq!(eof["eof_receipt"], flushed["receipt"]);
    assert!(eof.get("error").is_none());
    assert!(process.wait().unwrap().success());
    client.close();
    drop(client);
    service.shutdown();
    let store =
        spindle::custody::Store::open(&service.config.storage, service.config.quota_bytes).unwrap();
    let mut after = 0;
    let mut count = 0;
    while let Some((id, raw)) = store.read_raw(after, 1).unwrap().into_iter().next() {
        let record = observation::decode_versioned(&raw).unwrap();
        assert_eq!(record.context(), Some(&context));
        count += 1;
        after = id;
    }
    assert_eq!(count, 3);
}
