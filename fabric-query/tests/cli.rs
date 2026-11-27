use fabric::{
    canonical::{self, Request, Response},
    canonical_auth,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn non_tmp_dir() -> tempfile::TempDir {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let _ = std::fs::create_dir_all(&base);
    tempfile::tempdir_in(base).unwrap()
}

#[test]
fn help_flags_succeed_without_credentials_and_document_credential_file() {
    for flag in ["--help", "-h"] {
        let output = Command::new(env!("CARGO_BIN_EXE_fabric-query"))
            .arg(flag)
            .env_remove("FABRIC_ENDPOINT")
            .env_remove("FABRIC_CREDENTIAL")
            .env_remove("FABRIC_CREDENTIAL_FILE")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "fabric-query {flag} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("FABRIC_ENDPOINT"));
        assert!(stdout.contains("FABRIC_CREDENTIAL_FILE"));
        assert!(stdout.contains("FABRIC_CREDENTIAL"));
    }
}

#[test]
fn help_rejects_extra_arguments_without_credentials() {
    let output = Command::new(env!("CARGO_BIN_EXE_fabric-query"))
        .args(["--help", "unexpected"])
        .env_remove("FABRIC_ENDPOINT")
        .env_remove("FABRIC_CREDENTIAL")
        .env_remove("FABRIC_CREDENTIAL_FILE")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Usage:"));
}

#[test]
fn progress_and_capabilities_work_with_fabric_credential_file() {
    let dir = non_tmp_dir();
    let cred_path = dir.path().join("reader.key");
    std::fs::write(&cred_path, "owned-cli-reader-token\n").unwrap();
    std::fs::set_permissions(&cred_path, std::fs::Permissions::from_mode(0o600)).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_server = stop.clone();
    let server = std::thread::spawn(move || {
        while !stop_server.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let deadline = Instant::now() + Duration::from_secs(5);
                    let (credential_index, request) = canonical_auth::accept_authenticated_sync(
                        &mut stream,
                        ["owned-cli-reader-token"],
                        deadline,
                    )
                    .unwrap();
                    assert_eq!(credential_index, 0);
                    let response = match request {
                        Request::Progress => Response::Progress {
                            authority: "query-cli-test".into(),
                            frontiers: BTreeMap::from([("origin-a".into(), 42)]),
                        },
                        Request::Capabilities => Response::Capabilities {
                            authority: "query-cli-test".into(),
                            schemas: vec![1, 2],
                            origins: vec!["origin-a".into()],
                            max_frame: 1024 * 1024,
                            max_batch_records: 128,
                        },
                        _ => Response::Error {
                            error: "unexpected request".into(),
                        },
                    };
                    let _ = canonical::write_frame(
                        &mut stream,
                        &serde_json::to_vec(&response).unwrap(),
                        deadline,
                    );
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(e) => panic!("{e}"),
            }
        }
    });

    for (cmd, expected_key) in [("progress", "frontiers"), ("capabilities", "schemas")] {
        let output = Command::new(env!("CARGO_BIN_EXE_fabric-query"))
            .arg(cmd)
            .env("FABRIC_ENDPOINT", address.to_string())
            .env("FABRIC_CREDENTIAL_FILE", &cred_path)
            .env_remove("FABRIC_CREDENTIAL")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "fabric-query {cmd} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let parsed: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(parsed["authority"].as_str(), Some("query-cli-test"));
        assert!(
            parsed.get(expected_key).is_some(),
            "missing {expected_key} in {parsed}"
        );
    }

    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();
}
