mod companion;

use fabric_adapter_linux::operational_log::OperationalLog;
use fabric_server::{
    config::{Config, SpindleSettings},
    serve,
    store::CommitMode,
};
use std::io;
use std::process::ExitCode;
use std::time::{Duration, Instant};

async fn run(config: Config, settings: SpindleSettings, timing_events: bool) -> io::Result<()> {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    run_with_shutdown(
        config,
        settings,
        async move {
            tokio::select! { _ = term.recv() => {}, _ = interrupt.recv() => {} }
        },
        Duration::from_secs(12),
        timing_events,
    )
    .await
}

// A private shutdown future/deadline permits short tests of this same production
// supervisor; neither is a configuration or environment override.
async fn run_with_shutdown(
    config: Config,
    settings: SpindleSettings,
    shutdown: impl std::future::Future<Output = ()>,
    child_grace: Duration,
    timing_events: bool,
) -> io::Result<()> {
    std::fs::create_dir_all(&config.state_dir)?;
    let diagnostics = OperationalLog::open(&config.state_dir.join("diagnostics"), "server")?;
    diagnostics.event("starting")?;
    let local =
        companion::LocalSpindle::prepare(&config, settings, diagnostics.path().to_path_buf())?;
    tokio::pin!(shutdown);
    let handle = axum_server::Handle::new();
    let serving = serve(config, CommitMode::GROUPED, handle.clone());
    tokio::pin!(serving);
    let listening = handle.listening();
    tokio::pin!(listening);
    let mut child: Option<companion::Companion> = None;
    let mut bound = false;
    let mut stopping: Option<Instant> = None;
    let mut shutting_http = false;
    let mut failure = None;
    let mut diagnostic_failed = false;
    let mut next_sample = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let result = loop {
        tokio::select! {
            result=&mut serving=>{
                if result.is_ok() && let Some(companion)=&mut child {
                    match companion.0.try_wait()? {
                        Some(status) if status.success()=>{},
                        Some(status)=>{ failure.get_or_insert_with(||io::Error::other(
                            format!("dedicated Spindle shutdown failed: {status}"))); },
                        None=>{ failure.get_or_insert_with(||io::Error::other(
                            "HTTP serving ended before dedicated Spindle shutdown")); },
                    }
                }
                break result;
            },
            address=&mut listening, if !bound && stopping.is_none()=>{
                bound=true;
                let started=address.ok_or_else(||io::Error::other("server listener did not start"))
                    .and_then(|address|local.start(address, timing_events).map(|child|(address,child)));
                match started {
                    Ok((address,companion))=>{
                        eprintln!("fabric-server: listening on {address}; dedicated spindle pid={}",companion.0.id());
                        child=Some(companion);
                        let _=diagnostics.event("spindle_started");
                    },
                    Err(error)=>{
                        failure=Some(error);
                        stopping=Some(Instant::now());
                    },
                }
            },
            _=&mut shutdown, if stopping.is_none()=>{
                stopping=Some(Instant::now());
                if let Some(child)=&mut child { child.terminate()?; }
                let _=diagnostics.event("stopping");
            },
            _=tick.tick()=>{
                if let Some(companion)=&mut child
                    && let Some(status)=companion.0.try_wait()? {
                    if stopping.is_none() {
                        failure.get_or_insert_with(||io::Error::other(format!("dedicated Spindle exited unexpectedly: {status}")));
                        stopping=Some(Instant::now());
                        let _=diagnostics.event("spindle_exited");
                    } else if !status.success() {
                        failure.get_or_insert_with(||io::Error::other(format!("dedicated Spindle shutdown failed: {status}")));
                    }
                    child=None;
                }
                if let Some(started)=stopping {
                    if child.is_some() && started.elapsed()>=child_grace {
                        failure.get_or_insert_with(||io::Error::other("dedicated Spindle shutdown deadline exceeded; forced kill"));
                        child=None;
                    }
                    if child.is_none() && !shutting_http {
                        handle.graceful_shutdown(Some(Duration::from_secs(10)));
                        shutting_http=true;
                    }
                }
                if Instant::now()>=next_sample {
                    next_sample=Instant::now()+Duration::from_secs(15);
                    let failed=diagnostics.sample().is_err();
                    if failed!=diagnostic_failed {
                        eprintln!("fabric-server: local diagnostics {}",if failed {"unavailable"} else {"resumed"});
                        diagnostic_failed=failed;
                    }
                }
            },
        }
    };
    // HTTP/bind failure also owns child cleanup. Never leave it running after
    // this invocation returns; its durable Spool survives under server state.
    drop(child);
    let _ = diagnostics.event(if result.is_ok() && failure.is_none() {
        "stopped"
    } else {
        "failed"
    });
    failure.map_or(result, Err)
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if let [mode, path, principal] = args.as_slice()
        && mode == "recover-access"
    {
        return match recover_access(path, principal) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("fabric-server: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if let [mode, path] = args.as_slice()
        && mode == "renew-bootstrap"
    {
        return match recover_access(path, "") {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("fabric-server: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let Some((mode, path, timing_events)) = serving_args(&args) else {
        eprintln!(
            "usage: fabric-server serve|serve-legacy <CONFIG_PATH> [--timing-events] | renew-bootstrap <CONFIG_PATH> | recover-access <CONFIG_PATH> <OWNER_ID>"
        );
        return ExitCode::from(2);
    };
    let result = (|| {
        let (config, spindle) = Config::load_with_spindle(path)?;
        if mode == "serve" && config.access_origin.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "serve requires access_origin and access_rp_id; configure passkeys or explicitly use serve-legacy for migration",
            ));
        }
        if mode == "serve-legacy" {
            eprintln!(
                "fabric-server: explicit legacy administration mode; migrate to local passkeys before release acceptance"
            );
        }
        let runtime = tokio::runtime::Runtime::new()?;
        runtime.block_on(run(config, spindle, timing_events))
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fabric-server: {error}");
            ExitCode::FAILURE
        }
    }
}

fn serving_args(args: &[String]) -> Option<(&str, &str, bool)> {
    let (mode, path, timing) = match args {
        [mode, path] => (mode, path, false),
        [mode, path, flag] if flag == "--timing-events" => (mode, path, true),
        _ => return None,
    };
    matches!(mode.as_str(), "serve" | "serve-legacy").then_some((
        mode.as_str(),
        path.as_str(),
        timing,
    ))
}

/// Recovery is a local owner operation, serialized against the same lease held
/// by every production server and with a bounded private pre-change backup.
fn recover_access(path: &str, principal: &str) -> io::Result<()> {
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::unix::fs::OpenOptionsExt;
    let config = Config::load(path)?;
    let lease = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(config.state_dir.join("server-process.lock"))?;
    lease
        .try_lock()
        .map_err(|_| io::Error::other("stop the Fabric server before owner recovery"))?;
    let access_config = fabric_server::access::AccessConfig {
        origin: config
            .access_origin
            .clone()
            .ok_or_else(|| io::Error::other("access_origin required"))?,
        rp_id: config
            .access_rp_id
            .clone()
            .ok_or_else(|| io::Error::other("access_rp_id required"))?,
        audience: config.access_origin.clone().unwrap(),
    };
    if principal.is_empty() {
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        fabric_server::access::Access::renew_bootstrap_offline(
            &config.state_dir.join("access"),
            access_config,
            time,
        )?;
        eprintln!(
            "fabric-server: first-owner bootstrap renewed; read protected access/access-bootstrap.secret locally and enroll within ten minutes"
        );
        return Ok(());
    }
    let mut backup = std::collections::BTreeMap::new();
    for name in [
        "control.json",
        "access/access.json",
        "access/access.epoch",
        "access/access-bootstrap.secret",
    ] {
        let mut bytes = Vec::new();
        match File::open(config.state_dir.join(name)) {
            Ok(file) => {
                file.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
                if bytes.len() > 16 * 1024 * 1024 {
                    return Err(io::Error::other("recovery backup size cap"));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        backup.insert(name, bytes);
    }
    // Validate durable control before declaring an interrupted intent reconciled.
    fabric_server::control::Control::open(&config.state_dir)?;
    let control_hash = fabric_server::control::sha256_hex(&backup["control.json"]);
    let target = (0..2)
        .map(|i| config.state_dir.join(format!("owner-recovery-backup-{i}")))
        .find(|p| !p.exists())
        .ok_or_else(|| {
            io::Error::other(
                "export a prior owner recovery backup to free one of the two bounded backup slots",
            )
        })?;
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(&target)?;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(target.join("access"))?;
    for (name, bytes) in &backup {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(target.join(name))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    File::open(target.join("access"))?.sync_all()?;
    File::open(&target)?.sync_all()?;
    File::open(&config.state_dir)?.sync_all()?;
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    fabric_server::access::Access::recover_offline_reconciled(
        &config.state_dir.join("access"),
        access_config,
        principal,
        &control_hash,
        time,
    )?;
    eprintln!(
        "fabric-server: owner recovery prepared; read the protected access/access-bootstrap.secret file locally and enroll within ten minutes; prior workload credentials are invalidated"
    );
    Ok(())
}

#[cfg(test)]
mod shutdown_tests {
    // Origin: R2 soak review found that the supervisor ignored nonzero child
    // exits during shutdown and silently killed a child after its grace period.
    // Both could be reported as server success despite non-orderly shutdown.
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::Command;

    #[test]
    fn serving_timing_flag_is_explicit_and_cannot_change_recovery_commands() {
        let arguments = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        for mode in ["serve", "serve-legacy"] {
            let plain = arguments(&[mode, "server.conf"]);
            assert_eq!(serving_args(&plain), Some((mode, "server.conf", false)));
            let timed = arguments(&[mode, "server.conf", "--timing-events"]);
            assert_eq!(serving_args(&timed), Some((mode, "server.conf", true)));
        }
        for items in [
            vec!["serve", "server.conf", "--timing-event"],
            vec!["recover-access", "server.conf", "--timing-events"],
            vec!["renew-bootstrap", "server.conf", "--timing-events"],
            vec!["serve", "server.conf", "--timing-events", "extra"],
        ] {
            assert!(serving_args(&arguments(&items)).is_none());
        }
    }

    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                fs::remove_dir_all(&self.0).unwrap();
            }
        }
    }

    async fn supervised_shutdown(case: &str, handler: &str) -> io::Result<()> {
        let scratch = PathBuf::from(
            std::env::var_os("FABRIC_SCRATCH_ROOT")
                .expect("supervisor regression requires contained data-drive scratch"),
        );
        let fixture = Fixture(scratch.join(format!("shutdown-{}-{case}", std::process::id())));
        fs::create_dir(&fixture.0).unwrap();
        let certificate = Command::new("openssl")
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
            .current_dir(&fixture.0)
            .output()
            .unwrap();
        assert!(
            certificate.status.success(),
            "{}",
            String::from_utf8_lossy(&certificate.stderr)
        );
        fs::write(
            fixture.0.join("admin-token"),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n",
        )
        .unwrap();
        let executable = fixture.0.join("controlled-spindle");
        // One native process, with handlers installed before publishing readiness.
        // Its only effect is exercising supervisor ownership and shutdown status.
        fs::write(&executable, format!(
            "#!/usr/bin/python3\nimport os, pathlib, signal, sys, time\nsignal.signal(signal.SIGTERM, {handler})\npathlib.Path(sys.argv[2]).parent.joinpath('fixture-args').write_text(' '.join(sys.argv[1:]))\npathlib.Path(sys.argv[2]).parent.joinpath('fixture-ready').write_text(str(os.getpid()))\nwhile True: time.sleep(0.05)\n"
        )).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let config = Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            tls_cert: fixture.0.join("server.pem"),
            tls_key: fixture.0.join("server.key"),
            state_dir: fixture.0.join("state"),
            admin_token_file: fixture.0.join("admin-token"),
            console_dir: None,
            access_origin: None,
            access_rp_id: None,
            journal_bytes: 1024 * 1024,
            journal_file_bytes: 64 * 1024,
            retention_s: 86400,
            retention_bytes: 1024 * 1024,
            query_plan: fabric_server::query::Plan::Scan,
            seal_workers: 1,
        };
        let ready = config.state_dir.join("self-spindle/fixture-ready");
        let stopping_ready = ready.clone();
        let shutdown = async move {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !stopping_ready.is_file() {
                assert!(
                    Instant::now() < deadline,
                    "controlled Spindle did not become ready"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            run_with_shutdown(
                config,
                SpindleSettings {
                    executable: Some(executable),
                    ..Default::default()
                },
                shutdown,
                Duration::from_millis(200),
                case == "timing",
            ),
        )
        .await
        .expect("supervisor must finish and reap within the fixture deadline");
        let actual_args =
            fs::read_to_string(fixture.0.join("state/self-spindle/fixture-args")).unwrap();
        assert_eq!(actual_args.ends_with(" --timing-events"), case == "timing");
        let pid: libc::pid_t = fs::read_to_string(ready).unwrap().parse().unwrap();
        // SAFETY: signal zero only checks existence of the recorded child PID.
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "controlled child was not reaped"
        );
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        result
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn supervisor_shutdown_orderly_child_exit_is_success() {
        supervised_shutdown("orderly", "lambda *_: sys.exit(0)")
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn companion_timing_reaches_owned_child_without_changing_shutdown() {
        supervised_shutdown("timing", "lambda *_: sys.exit(0)")
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn supervisor_shutdown_nonzero_child_exit_is_failure() {
        let error = supervised_shutdown("nonzero", "lambda *_: sys.exit(7)")
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("dedicated Spindle shutdown failed"),
            "{error}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn supervisor_shutdown_forced_child_kill_is_failure() {
        let error = supervised_shutdown("ignores-term", "signal.SIG_IGN")
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("shutdown deadline exceeded; forced kill"),
            "{error}"
        );
    }
}
