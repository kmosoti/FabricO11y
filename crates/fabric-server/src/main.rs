use fabric_server::{config::Config, serve, store::CommitMode};
use std::process::ExitCode;
use std::time::Duration;

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [mode, config_path] = args.as_slice() else {
        eprintln!("usage: fabric-server serve <CONFIG_PATH>");
        return ExitCode::from(2);
    };
    if mode != "serve" {
        eprintln!("usage: fabric-server serve <CONFIG_PATH>");
        return ExitCode::from(2);
    }
    let config = match Config::load(config_path) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("fabric-server: {error}");
            return ExitCode::FAILURE;
        }
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("fabric-server: {error}");
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(async move {
        let handle = axum_server::Handle::new();
        let stopper = handle.clone();
        tokio::spawn(async move {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
            tokio::select! {
                _ = term.recv() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
            // In-flight requests finish; each waits for its durable answer.
            stopper.graceful_shutdown(Some(Duration::from_secs(10)));
        });
        eprintln!("fabric-server: listening on {}", config.listen);
        serve(config, CommitMode::GROUPED, handle).await
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fabric-server: {error}");
            ExitCode::FAILURE
        }
    }
}
