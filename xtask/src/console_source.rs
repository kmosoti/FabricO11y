//! Caller-pinned console build/source identity join. This is structural
//! provenance evidence only; it does not stage assets or grant acceptance.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

const SOURCE_LIMIT: usize = 512;
const PATH_LIMIT: usize = 4096;
const TRACK_QUERY_BYTES_LIMIT: usize = 512 * 1024;
const GIT_OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
const RAMFS_MAGIC: libc::c_long = 0x8584_58f6;

fn require(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.to_owned())
    }
}

fn valid_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn canonical_data_root() -> Result<PathBuf, String> {
    let configured =
        std::env::var_os("FABRIC_STORAGE_ROOT").ok_or("explicit FABRIC_STORAGE_ROOT required")?;
    let configured = PathBuf::from(configured);
    let metadata = std::fs::symlink_metadata(&configured).map_err(|error| error.to_string())?;
    require(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "FABRIC_STORAGE_ROOT must be an ordinary directory",
    )?;
    let root = configured
        .canonicalize()
        .map_err(|error| error.to_string())?;
    require(configured == root, "FABRIC_STORAGE_ROOT must be canonical")?;
    let descriptor = std::fs::File::open(&root).map_err(|error| error.to_string())?;
    let mut information = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: descriptor is a live directory and the output storage has the
    // exact UAPI type; read it only when fstatfs reports success.
    if unsafe { libc::fstatfs(descriptor.as_raw_fd(), information.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: successful fstatfs initialized the output structure.
    let information = unsafe { information.assume_init() };
    require(
        information.f_type != libc::TMPFS_MAGIC && information.f_type != RAMFS_MAGIC,
        "FABRIC_STORAGE_ROOT must be disk-backed DATA storage",
    )?;
    Ok(root)
}

fn admitted_directory(path: &Path, data: &Path, label: &str) -> Result<PathBuf, String> {
    require(path.is_absolute(), &format!("{label} must be absolute"))?;
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    require(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        &format!("{label} must be an ordinary directory"),
    )?;
    let canonical = path.canonicalize().map_err(|error| error.to_string())?;
    require(path == canonical, &format!("{label} must be canonical"))?;
    require(
        canonical.starts_with(data),
        &format!("{label} must be under DATA storage"),
    )?;
    Ok(canonical)
}

fn bounded_process(command: &mut Command, label: &str) -> Result<(ExitStatus, Vec<u8>), String> {
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|error| format!("{label} unavailable: {error}"))?;
    let mut stdout = child.stdout.take().ok_or("Git stdout pipe unavailable")?;
    let mut output = Vec::new();
    let mut buffer = [0u8; 16 * 1024];
    let mut too_large = false;
    let read_result = (|| -> Result<(), String> {
        loop {
            let count = stdout
                .read(&mut buffer)
                .map_err(|error| format!("{label} output read failed: {error}"))?;
            if count == 0 {
                break;
            }
            if output.len() + count <= GIT_OUTPUT_LIMIT {
                output.extend_from_slice(&buffer[..count]);
            } else {
                too_large = true;
            }
        }
        Ok(())
    })();
    let status = child
        .wait()
        .map_err(|error| format!("{label} wait failed: {error}"))?;
    read_result?;
    require(!too_large, "Git query output exceeds 4 MiB")?;
    Ok((status, output))
}

fn configure_git(command: &mut Command, root: &Path) {
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.excludesFile=/dev/null",
        ])
        .current_dir(root);
}

fn git(root: &Path, arguments: &[&str]) -> Result<(ExitStatus, Vec<u8>), String> {
    // The existing timeout utility supplies a hard deadline and reaps Git;
    // every query is from a fixed command set and no shell is involved.
    let mut command = Command::new("timeout");
    command.args(["--signal=KILL", "10s", "git", "--no-replace-objects"]);
    configure_git(&mut command, root);
    command.args(arguments);
    bounded_process(&mut command, "bounded Git query")
}

fn git_text(root: &Path, arguments: &[&str]) -> Result<String, String> {
    let (status, output) = git(root, arguments)?;
    require(
        status.success(),
        &format!("Git query failed or timed out: {:?}", status.code()),
    )?;
    String::from_utf8(output).map_err(|error| format!("Git returned non-UTF-8 output: {error}"))
}

fn canonical_paths(value: &Value) -> Result<BTreeMap<String, String>, String> {
    let sources = value
        .get("source_sha256")
        .and_then(Value::as_object)
        .ok_or("source_sha256 object required")?;
    require(
        !sources.is_empty() && sources.len() <= SOURCE_LIMIT,
        "source inventory must contain 1..=512 paths",
    )?;
    let mut result = BTreeMap::new();
    for (path, digest) in sources {
        require(
            path.len() <= PATH_LIMIT
                && !path.starts_with('/')
                && !path.contains('\0')
                && path.split('/').all(|part| !matches!(part, "" | "." | "..")),
            "source path must be canonical relative POSIX path",
        )?;
        let digest = digest.as_str().ok_or("source digest must be a string")?;
        require(valid_hex(digest, 64), "invalid source SHA-256")?;
        result.insert(path.clone(), digest.to_owned());
    }
    Ok(result)
}

fn tracked_sources(root: &Path, sources: &BTreeMap<String, String>) -> Result<(), String> {
    // A single literal-pathspec query bounds subprocess count independently of
    // inventory size; the inventory cap and per-path cap bound its argv.
    let argument_bytes = sources
        .keys()
        .try_fold(0usize, |total, path| total.checked_add(path.len() + 1))
        .ok_or("source path argument size overflow")?;
    require(
        argument_bytes <= TRACK_QUERY_BYTES_LIMIT,
        "source path arguments exceed bounded Git query size",
    )?;
    let mut command = Command::new("timeout");
    command.args(["--signal=KILL", "10s", "git", "--no-replace-objects"]);
    configure_git(&mut command, root);
    command
        .args([
            "--literal-pathspecs",
            "ls-files",
            "-v",
            "-z",
            "--error-unmatch",
            "--",
        ])
        .args(sources.keys());
    let (status, output) = bounded_process(&mut command, "bounded Git tracked-path query")?;
    require(
        status.success(),
        "source inventory contains an untracked or missing path",
    )?;
    let mut actual = Vec::new();
    for entry in output.split(|byte| *byte == 0) {
        if entry.is_empty() {
            continue;
        }
        require(
            entry.len() >= 3 && entry[1] == b' ',
            "malformed tracked path record",
        )?;
        require(
            entry[0] == b'H',
            "source index has assume-unchanged or skip-worktree flags",
        )?;
        actual.push(std::str::from_utf8(&entry[2..]).map_err(|_| "tracked path is not UTF-8")?);
    }
    require(
        actual.len() == sources.len(),
        "tracked source inventory mismatch",
    )?;
    require(
        actual
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            == sources.keys().map(String::as_str).collect(),
        "source inventory contains an untracked or missing path",
    )
}

fn source_identity(root: &Path, expected: &str) -> Result<(), String> {
    let top = PathBuf::from(git_text(root, &["rev-parse", "--show-toplevel"])?.trim());
    require(top == root, "source root is not the Git worktree root")?;
    require(
        git_text(root, &["rev-parse", "HEAD"])?.trim() == expected,
        "source HEAD does not match pinned commit",
    )?;
    require(
        git_text(root, &["status", "--porcelain", "--untracked-files=all"])?.is_empty(),
        "source checkout is not clean",
    )
}

/// Verify the explicit product checkout against an externally pinned build
/// receipt. The report is an identity join, not package acceptance.
pub fn verify(
    source_root: &Path,
    source_commit: &str,
    build_root: &Path,
    receipt_sha256: &str,
) -> Result<Value, String> {
    crate::preflight::require_limits()?;
    require(
        valid_hex(source_commit, 40),
        "source commit must be a full lowercase SHA-1",
    )?;
    require(valid_hex(receipt_sha256, 64), "receipt SHA-256 required")?;
    let data = canonical_data_root()?;
    let source_root = admitted_directory(source_root, &data, "source root")?;
    let build_root = admitted_directory(build_root, &data, "build root")?;

    source_identity(&source_root, source_commit)?;

    let mut receipt =
        crate::preflight::ArtifactStore::with_json_budget(&build_root, 4 * 1024 * 1024)?;
    receipt.read("build.json", receipt_sha256, true)?;
    let document = receipt.json("build.json")?;
    require(
        document.get("exit").and_then(Value::as_u64) == Some(0),
        "build receipt exit must be integer zero",
    )?;
    let sources = canonical_paths(document)?;
    tracked_sources(&source_root, &sources)?;

    let mut source = crate::preflight::ArtifactStore::new(&source_root)?;
    for (path, digest) in &sources {
        let _ = (path, digest);
    }
    source.recheck()?;
    receipt.recheck()?;
    source_identity(&source_root, source_commit)?;

    let hashed_bytes = source
        .bytes_hashed()
        .checked_add(receipt.bytes_hashed())
        .ok_or("hashed byte count overflow")?;
    Ok(json!({
        "status": "matched",
        "source_root": source_root,
        "source_commit": source_commit,
        "build_root": build_root,
        "receipt_sha256": receipt_sha256,
        "source_count": sources.len(),
        "source_bytes_hashed": source.bytes_hashed(),
        "receipt_bytes_hashed": receipt.bytes_hashed(),
        "bytes_hashed": hashed_bytes,
        "claim": "caller-pinned source and build receipt identity join; no acceptance credit"
    }))
}

pub fn cli(arguments: Vec<String>) -> u8 {
    let result = (|| {
        let mut values = BTreeMap::new();
        let mut arguments = arguments.into_iter();
        while let Some(flag) = arguments.next() {
            require(
                matches!(
                    flag.as_str(),
                    "--source-root" | "--source-commit" | "--build-root" | "--receipt-sha256"
                ),
                "unknown argument",
            )?;
            let value = arguments.next().ok_or("argument value required")?;
            require(!value.starts_with("--"), "argument value required")?;
            require(values.insert(flag, value).is_none(), "duplicate argument")?;
        }
        let source = values
            .remove("--source-root")
            .ok_or("--source-root required")?;
        let commit = values
            .remove("--source-commit")
            .ok_or("--source-commit required")?;
        let build = values
            .remove("--build-root")
            .ok_or("--build-root required")?;
        let receipt = values
            .remove("--receipt-sha256")
            .ok_or("--receipt-sha256 required")?;
        verify(Path::new(&source), &commit, Path::new(&build), &receipt)
    })();
    match result {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).unwrap_or_default()
            );
            0
        }
        Err(error) => {
            eprintln!("console source verification FAILED: {error}");
            2
        }
    }
}
