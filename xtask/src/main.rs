use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};
mod architecture;
mod benchmark;
mod coverage;
mod evidence;
mod extraction;
mod fuzz;
mod profile;
mod proof;
mod registry;
mod search;
#[cfg(test)]
mod workflow_tests;
fn output(cmd: &str, args: &[&str], dir: &Path) -> Result<String, String> {
    let p = Command::new(cmd)
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| e.to_string())?;
    if !p.status.success() {
        return Err(format!(
            "{cmd} {args:?}: {}\n{}",
            p.status,
            String::from_utf8_lossy(&p.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&p.stdout).trim().to_owned())
}
fn run(cmd: &str, args: &[&str], dir: &Path) -> Result<(), String> {
    let mut command = Command::new(cmd);
    command.args(args).current_dir(dir);
    let original = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    if cmd == "cargo" && dir != original {
        command.env(
            "CARGO_TARGET_DIR",
            dir.parent().unwrap().join("candidate-build"),
        );
    }
    let s = command.status().map_err(|e| e.to_string())?;
    if s.success() {
        Ok(())
    } else {
        Err(format!("{cmd} {args:?}: {s}"))
    }
}
fn capture(root: &Path) -> Result<String, String> {
    use std::os::unix::fs::PermissionsExt;
    let tool = env::split_paths(&env::var_os("PATH").ok_or("PATH missing")?)
        .map(|p| p.join("jj"))
        .find(|p| {
            p.is_file()
                && p.metadata()
                    .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
        })
        .ok_or("jj not found on PATH")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let identity = output(
        "sha256sum",
        &["--", tool.to_str().ok_or("non-UTF8 tool path")?],
        root,
    )?;

    output("jj", &["util", "snapshot"], root)?;
    let conflicts = output(
        "jj",
        &[
            "log",
            "--no-graph",
            "-r",
            "@ & conflicts()",
            "-T",
            "commit_id",
        ],
        root,
    )?;
    if !conflicts.is_empty() {
        return Err("candidate has conflicts".into());
    }
    Ok(format!(
        "{}\n{}\n{}\n{}\n",
        output(
            "jj",
            &["log", "--no-graph", "-r", "@", "-T", "commit_id"],
            root
        )?,
        output("rustc", &["--version", "--verbose"], root)?,
        output("jj", &["--version"], root)?,
        identity
    ))
}
fn capture_to(root: &Path, file: &Path) -> Result<(), String> {
    if !file.is_absolute() || file.starts_with(root) {
        return Err("capture must be an absolute path outside the source tree".into());
    }
    let file = evidence::artifact_path(file, root, false)?;
    let capture = capture(root)?;
    use std::io::Write;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(file)
        .and_then(|mut file| file.write_all(capture.as_bytes()))
        .map_err(|e| e.to_string())
}
fn checks(tier: &str, root: &Path) -> Result<(), String> {
    if !["fast", "medium", "extensive", "acceptance"].contains(&tier) {
        return Err("unknown tier".into());
    }
    coverage::validate(root)?;
    registry::check(root, false)?;
    run("cargo", &["fmt", "--all", "--", "--check"], root)?;
    if root.join("fuzz/Cargo.toml").is_file() {
        run(
            "cargo",
            &["fmt", "--manifest-path", "fuzz/Cargo.toml", "--", "--check"],
            root,
        )?;
    }
    run("cargo", &["check", "--workspace", "--locked"], root)?;
    run("cargo", &["test", "--workspace", "--locked"], root)?;
    if tier != "fast" {
        run(
            "cargo",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--locked",
                "--",
                "-D",
                "warnings",
            ],
            root,
        )?;
        run("cargo", &["test", "--doc", "--workspace", "--locked"], root)?;
        run("cargo", &["tree", "--locked"], root)?;
    }
    if tier == "extensive" {
        run(
            "cargo",
            &[
                "test",
                "-p",
                "xtask",
                "--locked",
                "workflow_",
                "--",
                "--ignored",
            ],
            root,
        )?;
    }
    if tier == "acceptance" {
        coverage::run(root)?;
        run(
            "cargo",
            &[
                "test",
                "-p",
                "xtask",
                "--locked",
                "--test",
                "collection_programs",
                "--",
                "--ignored",
                "--nocapture",
            ],
            root,
        )?;
        run(
            "cargo",
            &[
                "test",
                "-p",
                "xtask",
                "--locked",
                "--test",
                "distributed_programs",
                "--",
                "--ignored",
            ],
            root,
        )?;
        architecture::check(root)?;
        proof::check("all", root)?;
        proof::check("selftest", root)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let artifacts = root.join(format!(".local/acceptance-fuzz-{stamp}"));
        fuzz::cli(
            &[
                "replay",
                artifacts.to_str().ok_or("non-UTF8 artifact path")?,
            ],
            root,
        )?;
    }
    Ok(())
}
fn write_verification_status(root: &Path, receipt: &str) -> Result<(), String> {
    use std::io::Write;
    let directory = root.join(".local");
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let mut temporary = tempfile::NamedTempFile::new_in(&directory).map_err(|e| e.to_string())?;
    temporary
        .write_all(receipt.as_bytes())
        .map_err(|e| e.to_string())?;
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    temporary
        .persist(directory.join("last-verification"))
        .map_err(|e| e.to_string())?;
    Ok(())
}
fn verify(root: &Path, file: &Path) -> Result<(), String> {
    // Invalidate the prior successful receipt before any fallible capture work.
    // An interrupted attempt remains non-successful, even before a candidate is known.
    write_verification_status(root, "coverage=unknown\nresult=in-progress\n")?;
    let result = verify_candidate(root, file);
    if let Err(error) = &result {
        let detail = error.replace('\r', "\\r").replace('\n', "\\n");
        write_verification_status(
            root,
            &format!("coverage=unknown\nresult=failed: {detail}\n"),
        )?;
    }
    result
}
fn verify_candidate(root: &Path, file: &Path) -> Result<(), String> {
    let expected = fs::read_to_string(file).map_err(|e| e.to_string())?;
    if capture(root)? != expected {
        return Err("stale candidate or toolchain".into());
    }
    let id = expected.lines().next().ok_or("empty capture")?;
    if !xtask::valid_commit(id) {
        return Err("invalid commit identity".into());
    }
    let parent = file.parent().ok_or("capture has no parent")?;
    let work = parent.join(format!("verify-{id}"));
    if work.exists() {
        return Err("verification directory already exists; preserve or remove owned prior evidence explicitly".into());
    }
    output(
        "jj",
        &[
            "workspace",
            "add",
            "--no-colocate",
            "-r",
            id,
            work.to_str().ok_or("non-UTF8 path")?,
        ],
        root,
    )?;
    let mut coverage_manifest = None;
    let result: Result<(), String> = (|| {
        if !output(
            "jj",
            &["diff", "--from", id, "--to", "@", "--summary"],
            &work,
        )?
        .is_empty()
        {
            return Err("materialization differs".into());
        }
        coverage_manifest = Some(coverage::digest(&work)?);
        checks("acceptance", &work)?;
        output("jj", &["util", "snapshot"], &work)?;
        if !output(
            "jj",
            &["diff", "--from", id, "--to", "@", "--summary"],
            &work,
        )?
        .is_empty()
        {
            return Err("checks modified candidate".into());
        }
        if capture(root)? != expected {
            return Err("candidate changed during verification".into());
        }
        let artifacts = work.join(".local");
        if artifacts.exists() {
            let retained = parent.join(format!("evidence-{id}"));
            if retained.exists() {
                return Err("preserve existing candidate evidence".into());
            }
            fs::rename(artifacts, retained).map_err(|e| e.to_string())?;
        }
        output(
            "jj",
            &[
                "workspace",
                "forget",
                work.file_name().unwrap().to_str().unwrap(),
            ],
            root,
        )?;
        fs::remove_dir_all(&work).map_err(|e| e.to_string())?;
        Ok(())
    })();
    let receipt = format!(
        "candidate={id}\ncoverage_manifest={}\ncoverage={}\nresult={}\n",
        coverage_manifest.as_deref().unwrap_or("unavailable"),
        if result.is_ok() {
            "declared-required-tested"
        } else {
            "unknown"
        },
        match &result {
            Ok(()) => "success".to_owned(),
            Err(e) => format!("failed: {}", e.replace('\r', "\\r").replace('\n', "\\n")),
        }
    );
    fs::write(file.with_extension("receipt"), &receipt).map_err(|e| e.to_string())?;
    write_verification_status(root, &receipt)?;
    result
}
fn main() -> ExitCode {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let args: Vec<String> = env::args().skip(1).collect();
    let r = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["doctor"] => {
            for name in ["rustc", "cargo", "jj", "codex", "lean", "verus", "aeneas"] {
                println!("{name}: {:?}", output(name, &["--version"], &root));
            }
            Ok(())
        }
        ["check", "--tier", tier] => checks(tier, &root),
        ["collection", ..] => xtask::collection::run(&root, &args[1..]),
        ["prove"] => proof::check("verus", &root),
        ["prove", checker] => proof::check(checker, &root),
        ["architecture"] => architecture::check(&root),
        ["coverage", "validate"] => coverage::validate(&root),
        ["coverage", "check"] => coverage::run(&root),
        ["registry", "check"] => registry::check(&root, false),
        ["registry", "generate"] => registry::check(&root, true),
        ["search", rest @ ..] => search::cli(rest, &root),
        ["fuzz", rest @ ..] => fuzz::cli(rest, &root),
        ["profile", rest @ ..] => profile::cli(rest, &root),
        ["benchmark", "compare", rest @ ..] => benchmark::compare(rest),
        ["benchmark", out] => profile::cli(&["benchmark", out], &root),
        ["candidate", "capture", file] => capture_to(&root, Path::new(file)),
        ["candidate", "verify", file] => verify(&root, Path::new(file)),
        _ => Err(
            "usage: doctor | prove | check --tier fast|medium|extensive | candidate capture|verify FILE | coverage validate|check"
                .into(),
        ),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn invalid_tier_refused() {
        assert!(super::checks("unknown", std::path::Path::new(".")).is_err());
    }
    #[test]
    fn process_failure_retained() {
        assert!(super::output("/usr/bin/false", &[], std::path::Path::new(".")).is_err());
    }
    #[test]
    fn missing_capture_invalidates_previous_success_before_returning() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        super::write_verification_status(root, "candidate=old\nresult=success\n").unwrap();
        assert!(super::verify(root, &root.join("missing.capture")).is_err());
        let receipt = std::fs::read_to_string(root.join(".local/last-verification")).unwrap();
        assert!(receipt.contains("result=failed:"));
        assert!(receipt.contains("coverage=unknown"));
        assert!(!receipt.contains("result=success"));
        assert!(!receipt.contains("candidate=old"));
    }
    #[test]
    fn verification_status_replaces_success_with_in_progress_atomically() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        super::write_verification_status(root, "result=success\n").unwrap();
        super::write_verification_status(root, "coverage=unknown\nresult=in-progress\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join(".local/last-verification")).unwrap(),
            "coverage=unknown\nresult=in-progress\n"
        );
        assert_eq!(std::fs::read_dir(root.join(".local")).unwrap().count(), 1);
    }
}
