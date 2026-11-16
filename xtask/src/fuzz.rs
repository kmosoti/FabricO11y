use super::evidence;
use std::{
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
    time::Instant,
};

const TARGETS: [&str; 8] = [
    "admission_sequence",
    "bounded_encoding",
    "malformed_input",
    "observation_decode",
    "readback_envelope",
    "gorilla_ring_roundtrip",
    "unified_query_and_ipc",
    "causal_graph_and_wire",
];
const DEFAULT_NIGHTLY: &str = "nightly-2026-09-17";
const DEFAULT_BUILD_JOBS: u64 = 4;
const DEFAULT_SMOKE_SECONDS: u64 = 60;
const MAX_SMOKE_SECONDS: u64 = 3600;

fn fuzz_nightly() -> String {
    std::env::var("FABRIC_FUZZ_NIGHTLY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_NIGHTLY.to_owned())
}

fn parse_positive_u64(var: &str, raw: Option<&str>, default: u64) -> Result<u64, String> {
    match raw {
        Some(value) if !value.trim().is_empty() => value
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|&n| n >= 1)
            .ok_or_else(|| format!("invalid {var}: {value}")),
        _ => Ok(default),
    }
}

/// Parse a positive `u64` from environment variable `var`, returning `default` when unset or blank.
pub(crate) fn env_positive_u64(var: &str, default: u64) -> Result<u64, String> {
    match std::env::var(var) {
        Ok(raw) => parse_positive_u64(var, Some(&raw), default),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(_) => Err(format!("non-UTF8 {var}")),
    }
}

fn parse_smoke_seconds(raw: Option<&str>) -> Result<u64, String> {
    let seconds = parse_positive_u64("FABRIC_FUZZ_SMOKE_SECONDS", raw, DEFAULT_SMOKE_SECONDS)?;
    if seconds > MAX_SMOKE_SECONDS {
        return Err(format!(
            "FABRIC_FUZZ_SMOKE_SECONDS must be within 1..={MAX_SMOKE_SECONDS}, got {seconds}"
        ));
    }
    Ok(seconds)
}

fn fuzz_smoke_seconds() -> Result<u64, String> {
    match std::env::var("FABRIC_FUZZ_SMOKE_SECONDS") {
        Ok(raw) => parse_smoke_seconds(Some(&raw)),
        Err(std::env::VarError::NotPresent) => Ok(DEFAULT_SMOKE_SECONDS),
        Err(_) => Err("non-UTF8 FABRIC_FUZZ_SMOKE_SECONDS".into()),
    }
}

fn clean_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(
                    out.components().next_back(),
                    Some(Component::RootDir) | None
                ) {
                    out.pop();
                }
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

fn is_disallowed_ram_tmp(path: &Path) -> bool {
    path.as_os_str().is_empty()
        || path.starts_with("/tmp")
        || path.starts_with("/dev/shm")
        || path.starts_with("/run/shm")
}

fn validate_external_target_candidate(
    cleaned: &Path,
    root: &Path,
    canon_root: &Path,
) -> Result<(), String> {
    if is_disallowed_ram_tmp(cleaned) {
        return Err("FABRIC_FUZZ_TARGET_DIR must not be inside /tmp, /dev/shm, or /run/shm".into());
    }
    if cleaned.starts_with(root) || cleaned.starts_with(canon_root) {
        return Err("FABRIC_FUZZ_TARGET_DIR must be outside the source tree".into());
    }
    Ok(())
}

fn resolve_fuzz_target_dir(raw: Option<&str>, root: &Path) -> Result<Option<PathBuf>, String> {
    let Some(raw) = raw.filter(|s| !s.trim().is_empty()) else {
        return Ok(None);
    };
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err("FABRIC_FUZZ_TARGET_DIR must be an absolute path".into());
    }
    let cleaned = clean_lexical(path);
    let canon_root = root.canonicalize().map_err(|e| e.to_string())?;
    validate_external_target_candidate(&cleaned, root, &canon_root)?;

    // Resolve the deepest existing ancestor before creating directories so symlinks into
    // disallowed locations or the source tree are rejected before touching disk.
    let mut existing = cleaned.as_path();
    let mut suffix: Vec<OsString> = Vec::new();
    while existing.symlink_metadata().is_err() {
        let name = existing
            .file_name()
            .ok_or_else(|| "FABRIC_FUZZ_TARGET_DIR has no existing ancestor".to_owned())?;
        suffix.push(name.to_os_string());
        existing = existing
            .parent()
            .ok_or_else(|| "FABRIC_FUZZ_TARGET_DIR has no existing parent".to_owned())?;
    }
    let mut resolved = existing.canonicalize().map_err(|e| e.to_string())?;
    for part in suffix.into_iter().rev() {
        resolved.push(part);
    }
    validate_external_target_candidate(&resolved, root, &canon_root)?;

    fs::create_dir_all(&cleaned).map_err(|e| e.to_string())?;
    let canon = fs::canonicalize(&cleaned).map_err(|e| e.to_string())?;
    validate_external_target_candidate(&canon, root, &canon_root)?;
    Ok(Some(canon))
}

fn fuzz_target_dir(root: &Path) -> Result<Option<PathBuf>, String> {
    match std::env::var("FABRIC_FUZZ_TARGET_DIR") {
        Ok(raw) => resolve_fuzz_target_dir(Some(&raw), root),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => Err("non-UTF8 FABRIC_FUZZ_TARGET_DIR".into()),
    }
}

pub fn cli(args: &[&str], root: &Path) -> Result<(), String> {
    let (mode, out) = match args {
        [mode @ ("replay" | "smoke" | "scheduled"), out] => (*mode, Path::new(out)),
        _ => return Err("fuzz replay|smoke|scheduled ABSOLUTE_ARTIFACT_DIRECTORY".into()),
    };
    if !out.is_absolute() || out.exists() {
        return Err("use a new absolute artifact directory".into());
    }
    let out = evidence::artifact_path(out, root, true)?;
    let build_jobs = env_positive_u64("CARGO_BUILD_JOBS", DEFAULT_BUILD_JOBS)?.to_string();
    let shared_build_dir = fuzz_target_dir(root)?;
    let seconds = match mode {
        "replay" => 0,
        "smoke" => fuzz_smoke_seconds()?,
        _ => 1800,
    };
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
    let nightly = fuzz_nightly();
    let compiler = super::output(
        "rustup",
        &["run", &nightly, "rustc", "--version", "--verbose"],
        root,
    )?;
    let capture = super::capture(root)?;
    let build_dir = shared_build_dir.unwrap_or_else(|| out.join("build"));
    let build_log = fs::File::create(out.join("build.log")).map_err(|e| e.to_string())?;
    let build_status = Command::new("timeout")
        .arg("1200")
        .arg(&binary)
        .args(["fuzz", "build", "--codegen-units", "16"])
        .env("RUSTUP_TOOLCHAIN", &nightly)
        .env("CARGO_BUILD_JOBS", &build_jobs)
        .env("CARGO_TARGET_DIR", &build_dir)
        .current_dir(root)
        .stdout(build_log.try_clone().map_err(|e| e.to_string())?)
        .stderr(build_log)
        .status()
        .map_err(|e| e.to_string())?;
    if !build_status.success() {
        return Err(format!("cargo-fuzz build failed: {build_status}"));
    }
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
            .args(["fuzz", "run", "--codegen-units", "16", target])
            .arg(&corpus)
            .args(["--", "-max_len=65536", "-rss_limit_mb=2048", "-timeout=10"])
            .arg(format!("-artifact_prefix={}/", artifacts.display()));
        if mode == "replay" {
            command.arg("-runs=0");
        } else {
            command.arg(format!("-max_total_time={seconds}"));
        }
        // Single libFuzzer worker avoids parallel fuzz processes competing for RSS.
        command
            .env("RUSTUP_TOOLCHAIN", &nightly)
            .env("CARGO_BUILD_JOBS", &build_jobs)
            .env("CARGO_TARGET_DIR", &build_dir)
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
                "nightly": nightly, "cargo_fuzz": version, "tool_sha256": tool_hash,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_build_jobs_and_smoke_seconds_bounds() {
        assert_eq!(parse_positive_u64("CARGO_BUILD_JOBS", None, 4).unwrap(), 4);
        assert_eq!(
            parse_positive_u64("CARGO_BUILD_JOBS", Some(""), 4).unwrap(),
            4
        );
        assert_eq!(
            parse_positive_u64("CARGO_BUILD_JOBS", Some("8"), 4).unwrap(),
            8
        );
        assert!(parse_positive_u64("CARGO_BUILD_JOBS", Some("0"), 4).is_err());
        assert!(parse_positive_u64("CARGO_BUILD_JOBS", Some("two"), 4).is_err());

        assert_eq!(parse_smoke_seconds(None).unwrap(), 60);
        assert_eq!(parse_smoke_seconds(Some(" ")).unwrap(), 60);
        assert_eq!(parse_smoke_seconds(Some("1")).unwrap(), 1);
        assert_eq!(parse_smoke_seconds(Some("15")).unwrap(), 15);
        assert_eq!(parse_smoke_seconds(Some("3600")).unwrap(), 3600);
        assert!(parse_smoke_seconds(Some("0")).is_err());
        assert!(parse_smoke_seconds(Some("3601")).is_err());
        assert!(parse_smoke_seconds(Some("-1")).is_err());
        assert!(parse_smoke_seconds(Some("nan")).is_err());
    }

    #[test]
    fn fuzz_target_dir_enforces_external_non_ram_boundary() {
        let base = evidence::non_tmp_tempdir().unwrap();
        let root = base.path().join("repo");
        let external = base.path().join("external");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&external).unwrap();

        assert_eq!(resolve_fuzz_target_dir(None, &root).unwrap(), None);
        assert_eq!(resolve_fuzz_target_dir(Some(""), &root).unwrap(), None);
        assert!(resolve_fuzz_target_dir(Some("relative/path"), &root).is_err());

        for disallowed in [
            "/tmp/fuzz-target",
            "/dev/shm/fuzz-target",
            "/run/shm/fuzz-target",
            "/var/tmp/../../tmp/fuzz-target",
        ] {
            assert!(
                resolve_fuzz_target_dir(Some(disallowed), &root).is_err(),
                "expected rejection for {disallowed}"
            );
        }

        let inside = root.join("fuzz-target");
        assert!(resolve_fuzz_target_dir(Some(inside.to_str().unwrap()), &root).is_err());
        assert!(!inside.exists());

        let symlink_to_root = external.join("link-to-repo");
        std::os::unix::fs::symlink(&root, &symlink_to_root).unwrap();
        let via_symlink = symlink_to_root.join("nested/fuzz-target");
        assert!(resolve_fuzz_target_dir(Some(via_symlink.to_str().unwrap()), &root).is_err());
        assert!(!root.join("nested").exists());

        let valid = external.join("nested/fuzz-target");
        let resolved = resolve_fuzz_target_dir(Some(valid.to_str().unwrap()), &root)
            .unwrap()
            .expect("expected valid target dir");
        assert!(resolved.is_dir());
        assert_eq!(resolved, fs::canonicalize(&valid).unwrap());
    }
}
