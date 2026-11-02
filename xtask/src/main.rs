use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};
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
    capture(root).and_then(|v| fs::write(file, v).map_err(|e| e.to_string()))
}
fn checks(tier: &str, root: &Path) -> Result<(), String> {
    if !["fast", "medium", "extensive"].contains(&tier) {
        return Err("unknown tier".into());
    }
    run("cargo", &["fmt", "--all", "--", "--check"], root)?;
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
    Ok(())
}
fn verify(root: &Path, file: &Path) -> Result<(), String> {
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
    let result = (|| {
        if !output(
            "jj",
            &["diff", "--from", id, "--to", "@", "--summary"],
            &work,
        )?
        .is_empty()
        {
            return Err("materialization differs".into());
        }
        checks("medium", &work)?;
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
        Ok(())
    })();
    let receipt = format!(
        "candidate={id}\nresult={}\n",
        match &result {
            Ok(()) => "success".to_owned(),
            Err(e) => format!("failed: {e}"),
        }
    );
    fs::write(file.with_extension("receipt"), receipt).map_err(|e| e.to_string())?;
    if result.is_ok() {
        fs::create_dir_all(root.join(".local")).map_err(|e| e.to_string())?;
        fs::copy(
            file.with_extension("receipt"),
            root.join(".local/last-verification"),
        )
        .map_err(|e| e.to_string())?;
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
    }
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
        ["candidate", "capture", file] => capture_to(&root, Path::new(file)),
        ["candidate", "verify", file] => verify(&root, Path::new(file)),
        _ => Err(
            "usage: doctor | check --tier fast|medium|extensive | candidate capture|verify FILE"
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
}
