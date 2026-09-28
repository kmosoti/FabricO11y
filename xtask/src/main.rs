//! `cargo xtask <command>`; the alias lives in `.cargo/config.toml`.
//!
//! Exit status: 0 passed, 1 violations found, 2 the check could not run.
//! A check that could not run is reported as such and never as a pass.

use std::path::PathBuf;
use std::process::ExitCode;
use xtask::metadata::Options;

const USAGE: &str = "usage:
  cargo xtask check-layers      [--manifest-path PATH] [--policy PATH] [--declared-only] [--offline]
  cargo xtask check-core-purity [--manifest-path PATH] [--policy PATH] [--declared-only] [--offline]
  cargo xtask checks            [--profile fast|qualification] [--only ID] [--receipts DIR]
  cargo xtask mutants           [--only ID]
  cargo xtask check-counterexamples
  cargo xtask cargo-mutants";

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
    if command == "check-counterexamples" || command == "cargo-mutants" {
        let result = if command == "cargo-mutants" {
            xtask::evidence::cargo_mutants(&root, &root.join("xtask/cargo-mutants-equivalent.json"))
        } else {
            xtask::evidence::check_counterexamples(
                &root,
                &root.join("docs/formal/counterexamples.json"),
            )
        };
        return match result {
            Ok(violations) if violations.is_empty() => {
                println!("{command}: passed");
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
        };
    }
    if command == "checks" || command == "mutants" {
        return registry(&command, &root, args.collect());
    }
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

/// `checks` exits 0 when every selected check passed, 1 when any failed and 3
/// when none failed but an environment was unavailable (incomplete, never a
/// pass). `mutants` exits 0 only when every mutant was caught by its named test.
fn registry(command: &str, root: &std::path::Path, args: Vec<String>) -> ExitCode {
    let mut profile = "fast".to_owned();
    let mut only = None;
    let mut receipts = root.join("target/verification/receipts");
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--profile" => profile = args.next().unwrap_or_default(),
            "--only" => only = args.next(),
            "--receipts" => receipts = args.next().map(PathBuf::from).unwrap_or(receipts),
            _ => {
                eprintln!("unknown argument `{arg}`\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    if command == "mutants" {
        return match xtask::mutants::run_all(
            root,
            &root.join("xtask/mutants.json"),
            only.as_deref(),
        ) {
            Ok(true) => {
                println!("mutants: every mutant caught by its named checker");
                ExitCode::SUCCESS
            }
            Ok(false) => {
                println!("mutants: NOT all caught; see the table");
                ExitCode::from(1)
            }
            Err(error) => {
                eprintln!("mutants: NOT RUN: {error}");
                ExitCode::from(2)
            }
        };
    }
    use xtask::checks::Status;
    match xtask::checks::run(
        root,
        &root.join("xtask/checks.json"),
        &profile,
        only.as_deref(),
        &receipts,
    ) {
        Ok(Status::Passed) => {
            println!("checks: PASSED; receipts in {}", receipts.display());
            ExitCode::SUCCESS
        }
        Ok(Status::Failed) => {
            println!("checks: FAILED; receipts in {}", receipts.display());
            ExitCode::from(1)
        }
        Ok(Status::EnvironmentUnavailable) => {
            println!(
                "checks: INCOMPLETE (environment unavailable); receipts in {}",
                receipts.display()
            );
            ExitCode::from(3)
        }
        Err(error) => {
            eprintln!("checks: NOT RUN: {error}");
            ExitCode::from(2)
        }
    }
}
