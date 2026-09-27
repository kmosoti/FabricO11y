//! Fabric Server: authenticated, durable batch intake for Fabric Nodes.
pub mod config;
pub mod http;
pub mod store;

use axum_server::tls_rustls::RustlsConfig;
use std::io;
use std::sync::Arc;

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
    let credentials = Arc::new(config::load_credentials(&config.node_credentials)?);
    let store = store::Store::open(&config.state_dir, config.journal_bytes, mode)?;
    let intake = store.spawn()?;
    let tls = RustlsConfig::from_pem_file(&config.tls_cert, &config.tls_key).await?;
    let app = http::router(http::AppState {
        intake,
        credentials,
    });
    axum_server::bind_rustls(config.listen, tls)
        .handle(handle)
        .serve(app.into_make_service())
        .await
}
