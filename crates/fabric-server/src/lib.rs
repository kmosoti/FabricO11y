//! Fabric Server: authenticated, durable batch intake for Fabric Nodes.
pub mod config;
pub mod control;
pub mod http;
pub mod query;
pub mod rows;
pub mod sealer;
pub mod segment;
pub mod store;
mod tail;
pub mod text_filter;

use axum_server::tls_rustls::RustlsConfig;
use sha2::{Digest, Sha256};
use std::io;
use std::sync::{Arc, Mutex};

/// Install the ring provider once; rustls is built without a default.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Open state, start the commit thread and serve until `shutdown` resolves.
pub async fn serve(
    config: config::Config,
    mode: store::CommitMode,
    handle: axum_server::Handle,
) -> io::Result<()> {
    install_crypto_provider();
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
    let sealer_thread = sealer::spawn(
        config.state_dir.clone(),
        intake.clone(),
        sealer::Retention {
            max_age_s: config.retention_s,
            max_bytes: config.retention_bytes,
        },
        Arc::clone(&stop_sealer),
    )?;
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
        let _ = sealer_thread.join();
        let _ = commit_thread.join();
    })
    .await
    .map_err(|e| io::Error::other(e.to_string()))?;
    served
}
