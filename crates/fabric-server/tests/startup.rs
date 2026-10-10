//! Startup errors must release journal ownership for a same-process retry.
//! Origin: TLS loading followed worker spawn; its early return skipped sealer
//! stop/join, and the sealer's Intake kept the commit writer alive.
use fabric_server::{
    config::Config,
    query::Plan,
    store::{CommitMode, Store},
};
use serde_json::json;
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new(case: &str) -> Self {
        let root = PathBuf::from(
            std::env::var_os("FABRIC_SCRATCH_ROOT")
                .expect("startup regression requires contained data-drive scratch"),
        );
        let path = root.join(format!(
            "startup-{}-{}-{case}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn trace(scratch: &Scratch, value: &serde_json::Value) {
    let mut file = fs::File::create(scratch.0.join("trace.json")).unwrap();
    file.write_all(serde_json::to_string_pretty(value).unwrap().as_bytes())
        .unwrap();
    file.sync_all().unwrap();
    println!(
        "{}",
        json!({"startup_trace":value,"path":scratch.0.join("trace.json")})
    );
}
fn tls_failure_releases_writer(case: &str) {
    let scratch = Scratch::new(case);
    let generated = Command::new("openssl")
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
        .current_dir(&scratch.0)
        .output()
        .expect("openssl required for existing TLS integration tests");
    assert!(
        generated.status.success(),
        "certificate generation failed: {}",
        String::from_utf8_lossy(&generated.stderr)
    );
    match case {
        "missing_certificate" => fs::remove_file(scratch.0.join("server.pem")).unwrap(),
        "malformed_key" => fs::write(
            scratch.0.join("server.key"),
            b"this is not a PEM private key\n",
        )
        .unwrap(),
        _ => unreachable!(),
    }
    fs::write(
        scratch.0.join("admin-token"),
        b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n",
    )
    .unwrap();
    let config = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        tls_cert: scratch.0.join("server.pem"),
        tls_key: scratch.0.join("server.key"),
        state_dir: scratch.0.join("state"),
        admin_token_file: scratch.0.join("admin-token"),
        console_dir: None,
        access_origin: None,
        access_rp_id: None,
        journal_bytes: 1024 * 1024,
        journal_file_bytes: 64 * 1024,
        retention_s: 86400,
        retention_bytes: 1024 * 1024,
        query_plan: Plan::Scan,
        seal_workers: 1,
    };
    fabric_server::config::load_admin_token(&config.admin_token_file).unwrap();
    // Prove this fixture is reopenable before startup acquires any worker owner.
    drop(
        Store::open_with(
            &config.state_dir,
            config.journal_bytes,
            config.journal_file_bytes,
            CommitMode::GROUPED,
        )
        .unwrap(),
    );
    let mut observation = json!({"origin":"TLS setup error must not retain commit/sealer ownership",
        "case":case,"listen":config.listen.to_string(),"pre_start_store_open":true,"reopen_attempts":[]});
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let started = Instant::now();
    let result = runtime.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(5),
            fabric_server::serve(
                config.clone(),
                CommitMode::GROUPED,
                axum_server::Handle::new(),
            ),
        )
        .await
    });
    observation["serve_elapsed_ms"] = json!(started.elapsed().as_millis());
    let error = match result {
        Ok(Err(error)) => error,
        Ok(Ok(())) => {
            trace(&scratch, &observation);
            panic!("invalid TLS unexpectedly served successfully");
        }
        Err(_) => {
            trace(&scratch, &observation);
            panic!("TLS startup did not finish within5seconds");
        }
    };
    observation["serve_error"] =
        json!({"kind":format!("{:?}", error.kind()),"message":error.to_string()});
    trace(&scratch, &observation);
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut reopened = false;
    loop {
        match Store::open_with(
            &config.state_dir,
            config.journal_bytes,
            config.journal_file_bytes,
            CommitMode::GROUPED,
        ) {
            Ok(store) => {
                observation["reopen_attempts"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"opened":true}));
                drop(store);
                reopened = true;
            }
            Err(error) => observation["reopen_attempts"]
                .as_array_mut()
                .unwrap()
                .push(json!({
                "opened":false,"kind":format!("{:?}", error.kind()),"message":error.to_string()})),
        }
        trace(&scratch, &observation); // Record actual lock result BEFORE assertion.
        if reopened || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        reopened,
        "serve returned TLS error but its journal writer remains owned; see trace.json"
    );
}

#[test]
fn missing_certificate_startup_error_releases_writer_for_same_process_retry() {
    tls_failure_releases_writer("missing_certificate");
}
#[test]
fn malformed_key_startup_error_releases_writer_for_same_process_retry() {
    tls_failure_releases_writer("malformed_key");
}
