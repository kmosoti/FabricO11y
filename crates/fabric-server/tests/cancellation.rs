//! Cancellation must release journal ownership; ordinary bind failure is control.
//! Origin: serve's HTTP await precedes its explicit worker stop/join path.
use fabric_server::{
    config::Config,
    query::Plan,
    store::{CommitMode, Store},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    net::TcpListener,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new(case: &str) -> Self {
        let root = PathBuf::from(
            std::env::var_os("FABRIC_SCRATCH_ROOT").expect("contained data-drive scratch"),
        );
        let path = root.join(format!(
            "cancellation-{}-{}-{case}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn config(&self) -> Config {
        Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            tls_cert: self.0.join("server.pem"),
            tls_key: self.0.join("server.key"),
            state_dir: self.0.join("state"),
            admin_token_file: self.0.join("admin-token"),
            console_dir: None,
            access_origin: None,
            access_rp_id: None,
            journal_bytes: 1024 * 1024,
            journal_file_bytes: 64 * 1024,
            retention_s: 86400,
            retention_bytes: 1024 * 1024,
            query_plan: Plan::Scan,
            seal_workers: 1,
        }
    }
    fn prepare(&self) {
        fs::write(
            self.0.join("admin-token"),
            "cancellation-admin-0123456789abcdef0123456789abcdef\n",
        )
        .unwrap();
        let mut child = Command::new("openssl")
            .args([
                "req",
                "-x509",
                "-newkey",
                "ec",
                "-pkeyopt",
                "ec_paramgen_curve:P-256",
                "-nodes",
                "-keyout",
                "server.key",
                "-out",
                "server.pem",
                "-days",
                "2",
                "-subj",
                "/CN=127.0.0.1",
                "-addext",
                "subjectAltName=IP:127.0.0.1",
            ])
            .current_dir(&self.0)
            .stdout(Stdio::from(
                fs::File::create(self.0.join("openssl.stdout")).unwrap(),
            ))
            .stderr(Stdio::from(
                fs::File::create(self.0.join("openssl.stderr")).unwrap(),
            ))
            .spawn()
            .expect("openssl required by existing TLS integration tests");
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "fixture certificate generation failed");
                break;
            }
            if Instant::now() >= end {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("fixture certificate deadline exceeded");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let config = self.config();
        drop(
            Store::open_with(
                &config.state_dir,
                config.journal_bytes,
                config.journal_file_bytes,
                CommitMode::GROUPED,
            )
            .unwrap(),
        );
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn persist(scratch: &Scratch, trace: &Value) {
    let mut output = fs::File::create(scratch.0.join("trace.json")).unwrap();
    output
        .write_all(serde_json::to_string_pretty(trace).unwrap().as_bytes())
        .unwrap();
    output.sync_all().unwrap();
    println!(
        "{}",
        json!({"cancellation_trace":trace,"path":scratch.0.join("trace.json")})
    );
}
fn require_reopen(scratch: &Scratch, config: &Config, trace: &mut Value) {
    let start = Instant::now();
    let end = start + Duration::from_secs(3);
    let mut opened = false;
    trace["reopen_attempts"] = json!([]);
    loop {
        let outcome = match Store::open_with(
            &config.state_dir,
            config.journal_bytes,
            config.journal_file_bytes,
            CommitMode::GROUPED,
        ) {
            Ok(store) => {
                drop(store);
                opened = true;
                json!({"opened":true})
            }
            Err(error) => {
                json!({"opened":false,"kind":format!("{:?}",error.kind()),"message":error.to_string()})
            }
        };
        trace["reopen_attempts"]
            .as_array_mut()
            .unwrap()
            .push(outcome);
        trace["reopen_elapsed_ms"] = json!(start.elapsed().as_millis());
        persist(scratch, trace); // Preserve the actual lock error before assertion.
        if opened || Instant::now() >= end {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        opened,
        "terminated serve future still owns journal writer; see trace.json"
    );
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn aborting_listening_serve_releases_worker_owned_journal() {
    let scratch = Scratch::new("abort");
    scratch.prepare();
    let config = scratch.config();
    let mut trace = json!({"case":"abort_after_listening","pre_start_store_open":true,
        "origin":"HTTP await cancellation bypasses explicit stop/join"});
    persist(&scratch, &trace);
    let runtime = runtime();
    runtime.block_on(async {
        let handle = axum_server::Handle::new();
        let serving = handle.clone();
        let owned_config = config.clone();
        let task = tokio::spawn(async move {
            fabric_server::serve(owned_config, CommitMode::GROUPED, serving).await
        });
        let addr = tokio::time::timeout(Duration::from_secs(5), handle.listening())
            .await
            .unwrap()
            .expect("valid TLS fixture must reach listening");
        trace["listening"] = json!(addr.to_string());
        persist(&scratch, &trace);
        task.abort();
        let joined = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("abort must complete within2seconds");
        let cancelled = matches!(&joined, Err(error) if error.is_cancelled());
        trace["task_cancelled"] = json!(cancelled);
        trace["task_result"] = json!(format!("{joined:?}"));
        persist(&scratch, &trace);
        assert!(cancelled, "fixture must cancel a listening serve task");
        // Poll without blocking the executor needed by cancellation cleanup.
        tokio::task::block_in_place(|| require_reopen(&scratch, &config, &mut trace));
    });
}

#[test]
fn occupied_port_bind_error_releases_worker_owned_journal() {
    let scratch = Scratch::new("bind");
    scratch.prepare();
    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut config = scratch.config();
    config.listen = occupied.local_addr().unwrap();
    let mut trace = json!({"case":"occupied_port_bind_failure","pre_start_store_open":true,
        "occupied":config.listen.to_string()});
    persist(&scratch, &trace);
    let runtime = runtime();
    let result = runtime
        .block_on(async {
            tokio::time::timeout(
                Duration::from_secs(5),
                fabric_server::serve(
                    config.clone(),
                    CommitMode::GROUPED,
                    axum_server::Handle::new(),
                ),
            )
            .await
        })
        .expect("bind failure must return within5seconds");
    let error = result.expect_err("occupied listener must refuse bind");
    trace["serve_error"] = json!({"kind":format!("{:?}",error.kind()),"message":error.to_string()});
    persist(&scratch, &trace);
    assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
    require_reopen(&scratch, &config, &mut trace);
}
