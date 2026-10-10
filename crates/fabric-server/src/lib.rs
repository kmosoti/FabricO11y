//! Fabric Server: authenticated, durable batch intake for Fabric Nodes.
pub mod access;
pub mod config;
pub mod console;
pub mod control;
mod coupled_catalog;
pub mod http;
mod lifecycle_workers;
mod peer_admission;
pub mod query;
mod read_catalog;
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
    handle: axum_server::Handle<std::net::SocketAddr>,
) -> io::Result<()> {
    install_crypto_provider();
    std::fs::create_dir_all(&config.state_dir)?;
    let admin = if config.access_origin.is_none() {
        Some(config::load_admin_token(&config.admin_token_file)?)
    } else {
        None
    };
    // Validate fallible TLS input before any worker owns the journal. An early
    // TLS error after sealer startup would otherwise leave its Intake alive,
    // retaining the commit writer and preventing same-process startup retry.
    let tls = RustlsConfig::from_pem_file(&config.tls_cert, &config.tls_key).await?;
    // Validate protected access and the entire public asset identity before
    // starting workers. A configured console never falls back to master auth.
    let scoped_access = match (&config.access_origin, &config.access_rp_id) {
        (Some(origin), Some(rp_id)) => Some(access::Access::open(
            &config.state_dir.join("access"),
            access::AccessConfig {
                origin: origin.clone(),
                rp_id: rp_id.clone(),
                audience: origin.clone(),
            },
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        )?),
        (None, None) => None,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "access origin and RP ID must be paired",
            ));
        }
    };
    let assets = config
        .console_dir
        .as_ref()
        .map(|dir| console::Assets::load(dir))
        .transpose()?
        .unwrap_or_default();
    let control = Arc::new(Mutex::new(control::Control::open(&config.state_dir)?));
    let store = store::Store::open_with(
        &config.state_dir,
        config.journal_bytes,
        config.journal_file_bytes,
        mode,
    )?;
    let (intake, commit_thread) = store.spawn_joinable()?;
    let mut workers = lifecycle_workers::Workers::new(commit_thread, handle.clone());
    let sealer_thread = match sealer::spawn(
        config.state_dir.clone(),
        intake.clone(),
        sealer::Retention {
            max_age_s: config.retention_s,
            max_bytes: config.retention_bytes,
        },
        workers.stop_flag(),
        config.seal_workers,
    ) {
        Ok(worker) => worker,
        Err(error) => {
            // No router owns Intake yet. Release the final sender before
            // waiting for commit to drain; preserve the startup error if the
            // cleanup also reports a worker panic.
            drop(intake);
            if let Err(cleanup_error) = workers.finish().await {
                eprintln!("fabric-server: cleanup after startup error: {cleanup_error}");
            }
            return Err(error);
        }
    };
    workers.register_sealer(sealer_thread);
    let state = http::AppState {
        intake,
        control,
        admin_token_sha256: admin
            .map(|admin| Sha256::digest(admin.as_bytes()).into())
            .unwrap_or([0; 32]),
        history: Arc::new(query::History::with_plan(
            &config.state_dir,
            config.query_plan,
        )),
    };
    let app = match scoped_access {
        Some(access) => {
            peer_admission::wrap(console::Console::new(state, access, assets, &config).router())
        }
        None => http::router(state),
    };
    let served = axum_server::bind_rustls(config.listen, tls)
        .handle(handle)
        .serve(app.into_make_service_with_connect_info::<std::net::SocketAddr>())
        .await;
    // Normal shutdown awaits both joins. Cancellation instead drops the owner,
    // signaling the already-started cleanup without blocking this executor.
    let cleanup = workers.finish().await;
    match served {
        Err(error) => {
            if let Err(cleanup_error) = cleanup {
                eprintln!("fabric-server: cleanup after serve error: {cleanup_error}");
            }
            Err(error) // Keep the primary bind/serve error.
        }
        Ok(()) => cleanup,
    }
}
