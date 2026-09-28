//! Fabric Server: authenticated, durable batch intake for Fabric Nodes.
pub mod config;
pub mod control;
pub mod http;
pub mod store;

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
    let store = store::Store::open(&config.state_dir, config.journal_bytes, mode)?;
    let intake = store.spawn()?;
    let tls = RustlsConfig::from_pem_file(&config.tls_cert, &config.tls_key).await?;
    let app = http::router(http::AppState {
        intake,
        control,
        admin_token_sha256: Sha256::digest(admin.as_bytes()).into(),
    });
    axum_server::bind_rustls(config.listen, tls)
        .handle(handle)
        .serve(app.into_make_service())
        .await
}
