use super::evidence;
use std::{fs, path::Path, process::Command, time::Instant};
const TARGETS: [&str; 4] = [
    "admission_sequence",
    "bounded_encoding",
    "malformed_input",
    "observation_decode",
];
const NIGHTLY: &str = "nightly-2026-10-26";

pub fn cli(args: &[&str], root: &Path) -> Result<(), String> {
    let (mode, out) = match args {
        [mode @ ("replay" | "smoke" | "scheduled"), out] => (*mode, Path::new(out)),
        _ => return Err("fuzz replay|smoke|scheduled ABSOLUTE_ARTIFACT_DIRECTORY".into()),
    };
    if !out.is_absolute() || out.exists() {
        return Err("use a new absolute artifact directory".into());
    }
    let out = evidence::artifact_path(out, root, true)?;
    fs::create_dir(&out).map_err(|e| e.to_string())?;
    let binary = evidence::binary("FABRIC_CARGO_FUZZ", "cargo-fuzz-0.13.2/bin/cargo-fuzz");
    let version = super::output(
        binary.to_str().ok_or("non-UTF8 tool path")?,
        &["--version"],
        root,
    )?;
    if version.trim() != "cargo-fuzz 0.13.2" {
        return Err(format!("unexpected cargo-fuzz: {version}"));
    }
    let tool_hash = evidence::hash(&binary)?;
    let compiler = super::output(
        "rustup",
        &["run", NIGHTLY, "rustc", "--version", "--verbose"],
        root,
    )?;
    let capture = super::capture(root)?;
    let seconds = match mode {
        "replay" => 0,
        "smoke" => 60,
        _ => 1800,
    };
    let mut failures = Vec::new();
    for target in TARGETS {
        let artifacts = out.join(target);
        fs::create_dir(&artifacts).map_err(|e| e.to_string())?;
        let log = fs::File::create(artifacts.join("campaign.log")).map_err(|e| e.to_string())?;
        let corpus = artifacts.join("corpus");
        fs::create_dir(&corpus).map_err(|e| e.to_string())?;
        for entry in
            fs::read_dir(root.join("fuzz/corpus").join(target)).map_err(|e| e.to_string())?
        {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
                return Err("regression corpus must contain regular files".into());
            }
            fs::copy(entry.path(), corpus.join(entry.file_name())).map_err(|e| e.to_string())?;
        }
        let mut command = Command::new("timeout");
        command
            .arg((seconds + 300).to_string())
            .arg(&binary)
            .args(["fuzz", "run", target])
            .arg(&corpus)
            .args(["--", "-max_len=65536", "-rss_limit_mb=2048", "-timeout=10"])
            .arg(format!("-artifact_prefix={}/", artifacts.display()));
        if mode == "replay" {
            command.arg("-runs=0");
        } else {
            command.arg(format!("-max_total_time={seconds}"));
        }
        // One worker keeps campaigns within the initial two-worker ceiling.
        command
            .env("RUSTUP_TOOLCHAIN", NIGHTLY)
            .env("CARGO_BUILD_JOBS", "2")
            .env("CARGO_TARGET_DIR", out.join("build"))
            .current_dir(root)
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log);
        let start = Instant::now();
        let status = command.status().map_err(|e| e.to_string())?;
        let unchanged = super::capture(root)? == capture;
        evidence::write(
            &artifacts.join("result.json"),
            &serde_json::json!({
            "target": target, "mode": mode, "exit": status.code(), "success": status.success() && unchanged,
            "source_unchanged": unchanged, "compiler": compiler,
                "elapsed_ms": start.elapsed().as_millis(), "seconds": seconds, "workers": 1,
                "nightly": NIGHTLY, "cargo_fuzz": version, "tool_sha256": tool_hash,
                "repository_capture": capture,
            }),
        )?;
        if !status.success() || !unchanged {
            failures.push(target);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("fuzz failures: {failures:?}"))
    }
}
