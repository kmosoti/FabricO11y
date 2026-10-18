//! `cargo xtask <command>`; the alias lives in `.cargo/config.toml`.
//!
//! Exit status: 0 passed, 1 violations found, 2 the check could not run.
//! A check that could not run is reported as such and never as a pass.
//! `evidence-preflight` instead uses 0 matched, 3 pending and 2 invalid.

use std::path::PathBuf;
use std::process::ExitCode;
use xtask::metadata::Options;

/// Match the reference Git query deadline using the already-required Linux
/// coreutils tool. Failure or absence is an error, never an uncapped fallback.
fn receipt_git(root: &std::path::Path, arguments: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("timeout")
        .args(["--signal=KILL", "10s", "git"])
        .args(arguments)
        .current_dir(root)
        .output()
        .map_err(|error| format!("bounded Git query unavailable: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Git query failed or timed out: {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_owned())
        .map_err(|error| error.to_string())
}

fn verify_receipts(root: &std::path::Path, arguments: Vec<String>) -> ExitCode {
    let setup = (|| -> Result<_, String> {
        xtask::preflight::require_limits()?;
        let mut flags = std::collections::BTreeMap::new();
        let mut arguments = arguments.into_iter();
        while let Some(flag) = arguments.next() {
            if !matches!(flag.as_str(), "--profile" | "--receipts") {
                return Err(format!("unknown argument: {flag}"));
            }
            let value = arguments.next().ok_or("argument value required")?;
            if flags.insert(flag, value).is_some() {
                return Err("duplicate argument".into());
            }
        }
        let profile = flags.remove("--profile").ok_or("--profile required")?;
        if !matches!(profile.as_str(), "fast" | "extended") {
            return Err("invalid profile".into());
        }
        let supplied = flags
            .remove("--receipts")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target/verification/receipts"));
        // Resolve the Cargo target ancestor while leaving the selected root
        // itself for descriptor-relative O_NOFOLLOW admission.
        let parent = supplied
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        let receipts = parent
            .canonicalize()
            .map_err(|error| error.to_string())?
            .join(supplied.file_name().ok_or("named receipt root required")?);
        let storage = PathBuf::from(
            std::env::var_os("FABRIC_STORAGE_ROOT").ok_or("explicit DATA storage required")?,
        )
        .canonicalize()
        .map_err(|error| error.to_string())?;
        if !receipts.starts_with(&storage) {
            return Err("receipts must be on the admitted DATA storage".into());
        }
        Ok((profile, receipts))
    })();
    let (profile, receipts) = match setup {
        Ok(values) => values,
        Err(error) => {
            eprintln!("receipt verification NOT RUN: {error}");
            return ExitCode::from(2);
        }
    };
    let result: Result<serde_json::Value, String> = (|| {
        let commit = receipt_git(root, &["rev-parse", "HEAD"])?;
        if !receipt_git(root, &["status", "--porcelain"])?.is_empty() {
            return Err("candidate worktree changed".into());
        }
        let report = xtask::receipts::verify(root, &profile, &commit, &receipts)?;
        if receipt_git(root, &["rev-parse", "HEAD"])? != commit
            || !receipt_git(root, &["status", "--porcelain"])?.is_empty()
        {
            return Err("candidate source changed during verification".into());
        }
        Ok(report)
    })();
    match result {
        Ok(report) => match serde_json::to_string_pretty(&report) {
            Ok(report) => {
                println!("{report}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("receipt verification FAILED: {error}");
                ExitCode::from(1)
            }
        },
        Err(error) => {
            eprintln!("receipt verification FAILED: {error}");
            ExitCode::from(1)
        }
    }
}

const USAGE: &str = "usage:
  cargo xtask check-layers      [--manifest-path PATH] [--policy PATH] [--declared-only] [--offline]
  cargo xtask check-core-purity [--manifest-path PATH] [--policy PATH] [--declared-only] [--offline]
  cargo xtask checks            [--profile fast|qualification] [--only ID] [--receipts DIR]
  cargo xtask checks --profile fast --light-workers 2 --receipts NEW_DIR
  cargo xtask mutants           [--only ID]
  cargo xtask check-counterexamples
  cargo xtask cargo-mutants
  cargo xtask verify-receipts --profile fast|extended [--receipts DIR]
  cargo xtask console-source --source-root ABS --source-commit HEX40 --build-root ABS --receipt-sha256 HEX64
  cargo xtask storage-census [--owned-scratch ABS_PATH]
  cargo xtask storage-census --owner-binding --owner-label LABEL --admission ABS_PATH --admission-sha256 HEX -- <owner walkthrough argv>
  cargo xtask evidence-preflight --root PATH --plan RELATIVE --plan-sha256 HEX --candidate REVISION";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    if command == "evidence-preflight" {
        return ExitCode::from(xtask::preflight::cli(args.collect()));
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(PathBuf::from)
        .unwrap_or_default();
    if command == "storage-census" {
        return xtask::census::cli(args.collect());
    }
    if command == "console-source" {
        return ExitCode::from(xtask::console_source::cli(args.collect()));
    }
    if command == "verify-receipts" {
        return verify_receipts(&root, args.collect());
    }
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
    let mut profile_count = 0usize;
    let mut only = None;
    let mut only_count = 0usize;
    let mut receipts = root.join("target/verification/receipts");
    let mut receipts_missing = false;
    let mut receipts_count = 0usize;
    let mut light_workers = None;
    let mut light_workers_count = 0usize;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--profile" => {
                profile_count += 1;
                profile = args.next().unwrap_or_default();
            }
            "--only" => {
                only_count += 1;
                only = args.next();
            }
            "--receipts" => {
                receipts_count += 1;
                match args.next() {
                    Some(value) => receipts = PathBuf::from(value),
                    None => receipts_missing = true,
                }
            }
            "--light-workers" => {
                light_workers_count += 1;
                light_workers = args.next();
            }
            _ => {
                eprintln!("unknown argument `{arg}`\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    if command == "mutants" {
        if light_workers.is_some() || light_workers_count != 0 {
            eprintln!("--light-workers is only valid for checks --profile fast");
            return ExitCode::from(2);
        }
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
    let light = match light_workers_request(
        &profile,
        profile_count,
        only_count,
        receipts_missing,
        receipts_count,
        light_workers.as_deref(),
        light_workers_count,
    ) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("checks: NOT RUN: {error}");
            return ExitCode::from(2);
        }
    };
    let result = if light {
        xtask::checks::run_with_light_workers(root, &root.join("xtask/checks.json"), &receipts)
    } else {
        xtask::checks::run(
            root,
            &root.join("xtask/checks.json"),
            &profile,
            only.as_deref(),
            &receipts,
        )
    };
    match result {
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

fn light_workers_request(
    profile: &str,
    profile_count: usize,
    only_count: usize,
    receipts_missing: bool,
    receipts_count: usize,
    workers: Option<&str>,
    workers_count: usize,
) -> Result<bool, String> {
    if workers_count == 0 {
        return Ok(false);
    }
    if profile != "fast" || profile_count != 1 || only_count != 0 {
        return Err("--light-workers requires checks --profile fast without --only".into());
    }
    if workers_count != 1 || workers != Some("2") {
        return Err("--light-workers accepts exactly the value 2 once".into());
    }
    if receipts_missing || receipts_count != 1 {
        return Err("--light-workers requires exactly one explicit --receipts NEW_DIR".into());
    }
    Ok(true)
}

#[cfg(test)]
mod light_worker_cli_tests {
    use super::light_workers_request;

    fn valid() -> Result<bool, String> {
        light_workers_request("fast", 1, 0, false, 1, Some("2"), 1)
    }

    #[test]
    fn light_workers_is_explicit_and_strictly_bounded() {
        assert!(valid().unwrap());
        assert!(light_workers_request("fast", 0, 0, false, 1, Some("2"), 1).is_err());
        assert!(!light_workers_request("fast", 1, 0, false, 0, None, 0).unwrap());
        assert!(light_workers_request("extended", 1, 0, false, 1, Some("2"), 1).is_err());
        assert!(light_workers_request("fast", 1, 1, false, 1, Some("2"), 1).is_err());
        assert!(light_workers_request("fast", 2, 0, false, 1, Some("2"), 1).is_err());
        assert!(light_workers_request("fast", 1, 0, false, 0, Some("2"), 1).is_err());
        assert!(light_workers_request("fast", 1, 0, true, 1, Some("2"), 1).is_err());
        assert!(light_workers_request("fast", 1, 0, false, 2, Some("2"), 1).is_err());
        assert!(light_workers_request("fast", 1, 0, false, 1, Some("3"), 1).is_err());
    }
}
