//! Experiment-only server composition: release pending sealing at a recorded gate.
use axum_server::tls_rustls::RustlsConfig;
use fabric_server::{config, control, http, query, sealer, store};
use sha2::{Digest, Sha256};
use std::{
    io,
    path::PathBuf,
    sync::{Arc, Mutex, atomic::Ordering},
    time::Duration,
};
async fn serve(
    config: config::Config,
    gate: PathBuf,
    handle: axum_server::Handle<std::net::SocketAddr>,
) -> io::Result<()> {
    let mode = store::CommitMode::GROUPED;
    fabric_server::install_crypto_provider();
    std::fs::create_dir_all(&config.state_dir)?;
    let admin = config::load_admin_token(&config.admin_token_file)?;
    let control = Arc::new(Mutex::new(control::Control::open(&config.state_dir)?));
    let store = store::Store::open_with(
        &config.state_dir,
        config.journal_bytes,
        config.journal_file_bytes,
        mode,
    )?;
    let (intake, commit_thread) = store.spawn_joinable()?;
    let stop_sealer = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seal_state = config.state_dir.clone();
    let seal_intake = intake.clone();
    let seal_stop = Arc::clone(&stop_sealer);
    let workers = config.seal_workers;
    let retention = sealer::Retention {
        max_age_s: config.retention_s,
        max_bytes: config.retention_bytes,
    };
    let sealer_thread = std::thread::spawn(move || -> io::Result<()> {
        while !gate.is_file() && !seal_stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        while !seal_stop.load(Ordering::SeqCst) {
            sealer::pass(&seal_state, &seal_intake, retention, workers)?;
            for _ in 0..10 {
                if seal_stop.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        Ok(())
    });
    let tls = RustlsConfig::from_pem_file(&config.tls_cert, &config.tls_key).await?;
    let app = http::router(http::AppState {
        intake,
        control,
        admin_token_sha256: Sha256::digest(admin.as_bytes()).into(),
        history: Arc::new(query::History::with_plan(
            &config.state_dir,
            config.query_plan,
        )),
    });
    let served = axum_server::bind_rustls(config.listen, tls)
        .handle(handle)
        .serve(app.into_make_service())
        .await;
    // Every request has its answer. Stop sealing, then let the commit thread
    // drain and release the journal before returning.
    stop_sealer.store(true, std::sync::atomic::Ordering::SeqCst);
    tokio::task::spawn_blocking(move || {
        let sealing = sealer_thread
            .join()
            .map_err(|_| io::Error::other("sealer panicked"))?;
        let _ = commit_thread.join();
        sealing
    })
    .await
    .map_err(|e| io::Error::other(e.to_string()))??;
    served
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.as_slice() == ["--describe"] {
        println!(
            "{}",
            serde_json::json!({"run_mib":option_env!("FABRIC_RUN_MIB_EXPERIMENT").unwrap_or("16"),"query_plan":"walk selectable by config","sealing_gate":true})
        );
        return Ok(());
    }
    let [config_path, gate_path] = args.as_slice() else {
        return Err(io::Error::other("usage: coupled_c5_server CONFIG GATE"));
    };
    let config = config::Config::load(config_path)?;
    let handle = axum_server::Handle::new();
    let stopper = handle.clone();
    tokio::spawn(async move {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        tokio::select! { _ = term.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
        stopper.graceful_shutdown(Some(Duration::from_secs(10)));
    });
    serve(config, PathBuf::from(gate_path), handle).await
}
