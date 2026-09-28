//! `cargo xtask <command>`; the alias lives in `.cargo/config.toml`.
//!
//! Exit status: 0 passed, 1 violations found, 2 the check could not run.
//! A check that could not run is reported as such and never as a pass.

use std::path::PathBuf;
use std::process::ExitCode;
use xtask::metadata::Options;

const USAGE: &str = "usage: cargo xtask <check-layers|check-core-purity> \
[--manifest-path PATH] [--policy PATH] [--declared-only] [--offline]";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(PathBuf::from)
        .unwrap_or_default();
    let mut manifest = root.join("Cargo.toml");
    let mut policy = None;
    let mut options = Options {
        resolve: true,
        offline: false,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--manifest-path" => manifest = args.next().map(PathBuf::from).unwrap_or_default(),
            "--policy" => policy = args.next().map(PathBuf::from),
            "--declared-only" => options.resolve = false,
            "--offline" => options.offline = true,
            _ => {
                eprintln!("unknown argument `{arg}`\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let result = match command.as_str() {
        "check-layers" => xtask::check_layers(
            &manifest,
            &policy.unwrap_or_else(|| root.join("docs/architecture/layers.json")),
            options,
        ),
        "check-core-purity" => xtask::check_core_purity(
            &manifest,
            &policy.unwrap_or_else(|| root.join("docs/architecture/core-purity.json")),
            options,
        ),
        _ => {
            eprintln!("unknown command `{command}`\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let mode = if options.resolve {
        "declared+resolved"
    } else {
        "declared"
    };
    match result {
        Ok(violations) if violations.is_empty() => {
            println!(
                "{command}: passed ({mode} metadata, {})",
                manifest.display()
            );
            ExitCode::SUCCESS
        }
        Ok(violations) => {
            for violation in &violations {
                println!("{violation}");
            }
            println!("{command}: FAILED with {} violation(s)", violations.len());
            ExitCode::from(1)
        }
        Err(error) => {
            eprintln!("{command}: NOT RUN: {error}");
            ExitCode::from(2)
        }
    }
}
