//! Benchmark-only server entry point that selects the commit mode, for the
//! registered grouped-versus-individual comparison. Both modes use the same
//! two-sync frame commit; individual mode commits each batch as its own group.
use fabric_server::{config::Config, serve, store::CommitMode};
use std::process::ExitCode;
use std::time::Duration;

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [config_path, mode] = args.as_slice() else {
        eprintln!("usage: server_mode <CONFIG_PATH> <grouped|individual>");
        return ExitCode::from(2);
    };
    let mode = match mode.as_str() {
        "grouped" => CommitMode::GROUPED,
        "individual" => CommitMode::INDIVIDUAL,
        _ => {
            eprintln!("usage: server_mode <CONFIG_PATH> <grouped|individual>");
            return ExitCode::from(2);
        }
    };
    let config = match Config::load(config_path) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("server_mode: {error}");
            return ExitCode::FAILURE;
        }
    };
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let result = runtime.block_on(async move {
        let handle = axum_server::Handle::new();
        let stopper = handle.clone();
        tokio::spawn(async move {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
            term.recv().await;
            stopper.graceful_shutdown(Some(Duration::from_secs(10)));
        });
        serve(config, mode, handle).await
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("server_mode: {error}");
            ExitCode::FAILURE
        }
    }
}
