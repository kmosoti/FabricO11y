use fabric::canonical::{
    self, Authenticated, Request as CanonicalRequest, Response as CanonicalResponse,
};
use fabric::spindle::{Health, Reply};
use fabric_collect::DaemonResponse;
use fabric_core::DoctorReport;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let base =
            std::env::var_os("TMPDIR").map_or_else(|| PathBuf::from("/data/tmp"), PathBuf::from);
        let base = fabric_store::validate_non_tmp_path_on_disk(&base).unwrap();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = base.join(format!(
            "fabric-production-ready-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_fabric"));
        cmd.env("HOME", &self.0)
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("FABRIC_RUNTIME_DIR", &self.0)
            .env_remove("FABRIC_CONFIG_FILE")
            .env_remove("FABRIC_SERVER_ENDPOINT")
            .env_remove("FABRIC_READER_CREDENTIAL_FILE")
            .env_remove("SPINDLE_SOCKET")
            .arg("--data-dir")
            .arg(self.0.join("no-data"));
        cmd
    }
    fn assert_no_data(&self) {
        assert!(!self.0.join("no-data").exists());
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn unix_peer(
    path: &Path,
    respond: impl FnOnce(std::os::unix::net::UnixStream) + Send + 'static,
) -> JoinHandle<()> {
    let listener = UnixListener::bind(path).unwrap();
    listener.set_nonblocking(true).unwrap();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match listener.accept() {
                Ok((socket, _)) => {
                    respond(socket);
                    return;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "CLI did not connect to local peer"
                    );
                    thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("local accept: {e}"),
            }
        }
    })
}
fn doctor(path: &Path, healthy: bool, daemon_running: bool) -> JoinHandle<()> {
    unix_peer(path, move |mut socket| {
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut line = String::new();
        BufReader::new(socket.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap(),
            serde_json::json!({"op":"doctor"})
        );
        let report = DoctorReport {
            healthy,
            daemon_running,
            ..DoctorReport::default()
        };
        writeln!(
            socket,
            "{}",
            serde_json::to_string(&DaemonResponse::Doctor(report)).unwrap()
        )
        .unwrap();
    })
}
fn spindle(path: &Path, origin: &str) -> JoinHandle<()> {
    let origin = origin.to_owned();
    unix_peer(path, move |mut socket| {
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut header = [0; 4];
        socket.read_exact(&mut header).unwrap();
        let mut data = vec![0; u32::from_be_bytes(header) as usize];
        socket.read_exact(&mut data).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&data).unwrap(),
            serde_json::json!({"command":"status"})
        );
        let response = serde_json::to_vec(&Reply::Status {
            status: Health {
                origin,
                ..Health::default()
            },
        })
        .unwrap();
        socket
            .write_all(&(response.len() as u32).to_be_bytes())
            .unwrap();
        socket.write_all(&response).unwrap();
    })
}
fn spindle_progress(path: &Path, origin: &str, authority: Option<&str>) -> JoinHandle<()> {
    let origin = origin.to_owned();
    let authority = authority.map(str::to_owned);
    unix_peer(path, move |mut socket| {
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut header = [0; 4];
        socket.read_exact(&mut header).unwrap();
        let mut data = vec![0; u32::from_be_bytes(header) as usize];
        socket.read_exact(&mut data).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&data).unwrap(),
            serde_json::json!({"command":"fabric_progress"})
        );
        let response = serde_json::to_vec(&Reply::FabricProgress {
            authority,
            origin,
            through: 0,
        })
        .unwrap();
        socket
            .write_all(&(response.len() as u32).to_be_bytes())
            .unwrap();
        socket.write_all(&response).unwrap();
    })
}
fn run_local(scratch: &Scratch, healthy: bool, running: bool, extra: &[&str]) -> Output {
    let collector = doctor(&scratch.0.join("daemon.sock"), healthy, running);
    let spool = spindle(&scratch.0.join("spindle.sock"), "origin-local");
    let output = scratch
        .command()
        .args(["production-ready", "--spindle-socket"])
        .arg(scratch.0.join("spindle.sock"))
        .args(extra)
        .output()
        .unwrap();
    collector.join().unwrap();
    spool.join().unwrap();
    scratch.assert_no_data();
    output
}
fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn missing_services_are_unavailable_without_creating_state() {
    let scratch = Scratch::new();
    let output = scratch
        .command()
        .args(["production-ready", "--spindle-socket"])
        .arg(scratch.0.join("absent.sock"))
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let data = &json(&output)["data"];
    assert_eq!(data["version"], 1);
    assert_eq!(data["status"], "unavailable");
    assert_eq!(data["collector"]["reason"], "collector_unavailable");
    assert_eq!(data["spindle"]["reason"], "spindle_unavailable");
    scratch.assert_no_data();
}

#[test]
fn standalone_ignores_broken_canonical_hints_and_requires_resident_healthy_doctor() {
    for (healthy, running, code, reason) in [
        (true, true, 0, "collector_healthy"),
        (false, true, 2, "collector_unhealthy"),
        (true, false, 3, "collector_not_resident"),
    ] {
        let scratch = Scratch::new();
        let dir = scratch.0.join("config/fabrico11y");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("server.toml"), "this is not valid TOML = [").unwrap();
        fs::write(
            dir.join("spindle.json"),
            serde_json::to_vec(&serde_json::json!({
                "socket":scratch.0.join("spindle.sock"), "forwarding":"invalid canonical settings"
            }))
            .unwrap(),
        )
        .unwrap();
        let collector = doctor(&scratch.0.join("daemon.sock"), healthy, running);
        let spool = spindle(&scratch.0.join("spindle.sock"), "origin-local");
        let output = scratch
            .command()
            .env("FABRIC_SERVER_ENDPOINT", "https://invalid.example")
            .arg("production-ready")
            .arg("--json")
            .output()
            .unwrap();
        collector.join().unwrap();
        spool.join().unwrap();
        assert_eq!(output.status.code(), Some(code), "{output:?}");
        let result = json(&output);
        assert_eq!(result["data"]["collector"]["reason"], reason);
        assert_eq!(result["data"]["canonical"]["status"], "not_requested");
        scratch.assert_no_data();
    }
}

fn canonical_peer(origin: &'static str, authority: &'static str) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    listener.set_nonblocking(true).unwrap();
    let worker = thread::spawn(move || {
        let end = Instant::now() + Duration::from_secs(10);
        let mut socket = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < end);
                    thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("canonical accept: {e}"),
            }
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        let request: Authenticated =
            serde_json::from_slice(&canonical::read_frame(&mut socket, deadline).unwrap()).unwrap();
        assert_eq!(request.credential, "reader-credential-12345");
        assert!(matches!(request.request, CanonicalRequest::Progress));
        let response = CanonicalResponse::Progress {
            authority: authority.into(),
            frontiers: BTreeMap::from([(origin.to_owned(), 0)]),
        };
        canonical::write_frame(
            &mut socket,
            &serde_json::to_vec(&response).unwrap(),
            deadline,
        )
        .unwrap();
    });
    (address, worker)
}

#[test]
fn canonical_requires_exact_origin_even_if_another_origin_is_present() {
    for (response_origin, code, status, frontier) in [
        ("origin-local", 0, "ready", Some(0)),
        ("origin-other", 2, "not_ready", None),
    ] {
        let scratch = Scratch::new();
        let collector = doctor(&scratch.0.join("daemon.sock"), true, true);
        let spool = spindle_progress(
            &scratch.0.join("spindle.sock"),
            "origin-local",
            Some("authority-test"),
        );
        let (endpoint, peer) = canonical_peer(response_origin, "authority-test");
        let credential = scratch.0.join("reader.key");
        fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&credential)
            .unwrap()
            .write_all(b"reader-credential-12345")
            .unwrap();
        let output = scratch
            .command()
            .args(["production-ready", "--spindle-socket"])
            .arg(scratch.0.join("spindle.sock"))
            .args(["--require-canonical", "--server-endpoint"])
            .arg(endpoint)
            .arg("--reader-credential-file")
            .arg(credential)
            .arg("--json")
            .output()
            .unwrap();
        collector.join().unwrap();
        spool.join().unwrap();
        peer.join().unwrap();
        assert_eq!(output.status.code(), Some(code), "{output:?}");
        let result = json(&output);
        assert_eq!(result["data"]["status"], status);
        assert_eq!(result["data"]["canonical_authority"], "authority-test");
        assert_eq!(result["data"]["canonical_frontier"].as_u64(), frontier);
        if code == 2 {
            assert_eq!(
                result["data"]["canonical"]["reason"],
                "canonical_origin_missing"
            );
        }
        assert!(!String::from_utf8_lossy(&output.stdout).contains("reader-credential-12345"));
        scratch.assert_no_data();
    }
}

#[test]
fn profile_socket_routing_and_json_table_arrow_are_supported() {
    let scratch = Scratch::new();
    let profile = scratch.0.join("profile.toml");
    fs::write(
        &profile,
        format!(
            "version = 1\n[daemon]\nspindle_socket = {:?}\n",
            scratch.0.join("profile.sock").display().to_string()
        ),
    )
    .unwrap();
    let collector = doctor(&scratch.0.join("daemon.sock"), true, true);
    let spool = spindle(&scratch.0.join("profile.sock"), "origin-profile");
    let output = scratch
        .command()
        .arg("--config")
        .arg(profile)
        .args(["production-ready", "--json"])
        .output()
        .unwrap();
    collector.join().unwrap();
    spool.join().unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(json(&output)["data"]["spindle_origin"], "origin-profile");
    scratch.assert_no_data();

    let scratch = Scratch::new();
    let profile = scratch.0.join("profile.toml");
    fs::write(
        &profile,
        format!(
            "version = 1\n[daemon]\nspindle_socket = {:?}\n",
            scratch.0.join("unused-profile.sock").display().to_string()
        ),
    )
    .unwrap();
    let collector = doctor(&scratch.0.join("daemon.sock"), true, true);
    let spool = spindle(&scratch.0.join("override.sock"), "origin-override");
    let output = scratch
        .command()
        .arg("--config")
        .arg(profile)
        .env("SPINDLE_SOCKET", scratch.0.join("override.sock"))
        .args(["production-ready", "--json"])
        .output()
        .unwrap();
    collector.join().unwrap();
    spool.join().unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(json(&output)["data"]["spindle_origin"], "origin-override");
    scratch.assert_no_data();

    let scratch = Scratch::new();
    let output = run_local(&scratch, true, true, &["--table"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("collector_healthy")
    );
    let scratch = Scratch::new();
    let output = run_local(&scratch, true, true, &["--arrow"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let mut stream =
        arrow::ipc::reader::StreamReader::try_new(std::io::Cursor::new(output.stdout), None)
            .unwrap();
    let batch = stream.next().unwrap().unwrap();
    assert_eq!(batch.num_rows(), 1);
    assert_eq!(
        batch
            .schema()
            .field_with_name("canonical_frontier")
            .unwrap()
            .data_type(),
        &arrow::datatypes::DataType::UInt64
    );
}

#[test]
fn resident_collector_doctor_is_checked_from_the_running_daemon() {
    struct Daemon(std::process::Child);
    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let scratch = Scratch::new();
    let tmp = std::env::var_os("TMPDIR").unwrap_or_else(|| "/data/tmp".into());
    let child = scratch
        .command()
        .arg("--tmp-dir")
        .arg(tmp)
        .args([
            "daemon",
            "--interval-ms",
            "1000",
            "--max-ticks",
            "30",
            "--spindle-socket",
        ])
        .arg(scratch.0.join("no-service.sock"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut daemon = Daemon(child);
    let deadline = Instant::now() + Duration::from_secs(25);
    while !scratch.0.join("daemon.sock").exists() {
        assert!(
            daemon.0.try_wait().unwrap().is_none(),
            "resident collector exited before readiness"
        );
        assert!(
            Instant::now() < deadline,
            "resident collector did not start"
        );
        thread::sleep(Duration::from_millis(50));
    }
    let spool = spindle(&scratch.0.join("spindle.sock"), "origin-live");
    let output = scratch
        .command()
        .args(["production-ready", "--spindle-socket"])
        .arg(scratch.0.join("spindle.sock"))
        .arg("--json")
        .output()
        .unwrap();
    spool.join().unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        json(&output)["data"]["collector"]["reason"],
        "collector_healthy"
    );
}

#[test]
fn canonical_credentials_require_an_owned_private_nonsymlink_file() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new();
    let credential = scratch.0.join("reader.key");
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&credential)
        .unwrap()
        .write_all(b"reader-credential-12345")
        .unwrap();
    let link = scratch.0.join("reader-link.key");
    symlink(&credential, &link).unwrap();
    let collector = doctor(&scratch.0.join("daemon.sock"), true, true);
    let spool = spindle_progress(
        &scratch.0.join("spindle.sock"),
        "origin-local",
        Some("authority-test"),
    );
    let output = scratch
        .command()
        .args(["production-ready", "--spindle-socket"])
        .arg(scratch.0.join("spindle.sock"))
        .args([
            "--require-canonical",
            "--server-endpoint",
            "127.0.0.1:9",
            "--reader-credential-file",
        ])
        .arg(link)
        .arg("--json")
        .output()
        .unwrap();
    collector.join().unwrap();
    spool.join().unwrap();
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert_eq!(
        json(&output)["data"]["canonical"]["reason"],
        "canonical_credential_unavailable"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("reader-credential-12345"));
    scratch.assert_no_data();
}

#[test]
fn fleet_requires_configured_authority_and_rejects_other_authority_for_same_origin() {
    for configured in [None, Some("expected-authority")] {
        let scratch = Scratch::new();
        let collector = doctor(&scratch.0.join("daemon.sock"), true, true);
        let spool = spindle_progress(&scratch.0.join("spindle.sock"), "origin-local", configured);
        let credential = scratch.0.join("reader.key");
        fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&credential)
            .unwrap()
            .write_all(b"reader-credential-12345")
            .unwrap();
        let (endpoint, peer) = if configured.is_some() {
            let (address, peer) = canonical_peer("origin-local", "other-authority");
            (address, Some(peer))
        } else {
            ("127.0.0.1:9".to_owned(), None)
        };
        let output = scratch
            .command()
            .args(["production-ready", "--spindle-socket"])
            .arg(scratch.0.join("spindle.sock"))
            .args(["--require-canonical", "--server-endpoint"])
            .arg(endpoint)
            .arg("--reader-credential-file")
            .arg(credential)
            .arg("--json")
            .output()
            .unwrap();
        collector.join().unwrap();
        spool.join().unwrap();
        if let Some(peer) = peer {
            peer.join().unwrap();
        }
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        let result = json(&output);
        assert_eq!(
            result["data"]["canonical"]["reason"],
            if configured.is_some() {
                "canonical_authority_mismatch"
            } else {
                "spindle_authority_unconfigured"
            }
        );
        assert!(result["data"]["canonical_frontier"].is_null());
        if configured.is_some() {
            assert_eq!(result["data"]["expected_authority"], "expected-authority");
            assert_eq!(result["data"]["canonical_authority"], "other-authority");
        }
        scratch.assert_no_data();
    }
}

fn rejected_doctor_reply(kind: &str) {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let scratch = Scratch::new();
    let completed = Arc::new(AtomicBool::new(false));
    let peer_completed = Arc::clone(&completed);
    let kind_owned = kind.to_owned();
    let peer = unix_peer(&scratch.0.join("daemon.sock"), move |mut socket| {
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = String::new();
        BufReader::new(socket.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&request).unwrap(),
            serde_json::json!({"op":"doctor"})
        );
        let mut report = DoctorReport {
            healthy: true,
            daemon_running: true,
            ..DoctorReport::default()
        };
        if kind_owned == "oversize" {
            report.checks.push(fabric_core::DoctorCheck {
                name: "large_valid_check".into(),
                passed: true,
                actual: "ok".into(),
                threshold: "ok".into(),
                detail: "x".repeat(64 * 1024),
            });
        }
        let mut bytes = serde_json::to_vec(&DaemonResponse::Doctor(report)).unwrap();
        // The rejected bytes would otherwise be a typed, healthy Doctor response.
        let decoded: DaemonResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(matches!(decoded, DaemonResponse::Doctor(d) if d.healthy && d.daemon_running));
        if kind_owned == "oversize" {
            assert!(bytes.len() > 64 * 1024);
        }
        bytes.push(b'\n');
        match kind_owned.as_str() {
            "incomplete" => {
                socket.write_all(b"{\"kind\":\"doctor\"").unwrap();
            }
            "trickle" => {
                // Every idle interval is well below the old 750 ms per-read limit,
                // but the complete valid reply takes longer than the total deadline.
                let mut all_sent = true;
                for chunk in bytes.chunks(bytes.len().div_ceil(8)) {
                    if socket.write_all(chunk).is_err() {
                        all_sent = false;
                        break;
                    }
                    thread::sleep(Duration::from_millis(200));
                }
                if all_sent {
                    peer_completed.store(true, Ordering::SeqCst);
                }
            }
            "oversize" => {
                if socket.write_all(&bytes).is_ok() {
                    peer_completed.store(true, Ordering::SeqCst);
                }
            }
            _ => unreachable!(),
        }
    });
    let spool = spindle(&scratch.0.join("spindle.sock"), "origin-local");
    let start = Instant::now();
    let output = scratch
        .command()
        .args(["production-ready", "--spindle-socket"])
        .arg(scratch.0.join("spindle.sock"))
        .arg("--json")
        .output()
        .unwrap();
    let elapsed = start.elapsed();
    peer.join().unwrap();
    spool.join().unwrap();
    assert_eq!(output.status.code(), Some(3), "{kind}: {output:?}");
    assert_eq!(
        json(&output)["data"]["collector"]["reason"],
        "collector_unavailable"
    );
    assert!(elapsed < Duration::from_secs(4), "{kind}: {elapsed:?}");
    if kind == "trickle" {
        assert!(
            !completed.load(Ordering::SeqCst),
            "readiness waited for the full trickled reply"
        );
    }
    scratch.assert_no_data();
}

#[test]
fn incomplete_doctor_reply_is_unavailable() {
    rejected_doctor_reply("incomplete");
}
#[test]
fn valid_trickled_doctor_reply_respects_total_deadline() {
    rejected_doctor_reply("trickle");
}
#[test]
fn valid_oversize_doctor_reply_is_unavailable() {
    rejected_doctor_reply("oversize");
}
