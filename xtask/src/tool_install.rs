//! Install one explicitly pinned CI tool under the repository's DATA and
//! resource-containment contract. Cargo remains the package installer.

use serde_json::{Value, json};
use std::ffi::{CString, OsStr};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const INSTALL_TIMEOUT: Duration = Duration::from_secs(1500);
const MIN_FREE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug)]
struct InstallError {
    status: u8,
    message: String,
}

impl InstallError {
    fn not_run(message: impl Into<String>) -> Self {
        Self {
            status: 2,
            message: message.into(),
        }
    }

    fn failed(message: impl Into<String>) -> Self {
        Self {
            status: 1,
            message: message.into(),
        }
    }
}

fn scratch_target(root: &Path) -> Result<PathBuf, String> {
    for sequence in 0..1000_u32 {
        let path = root.join(format!("cargo-tool-{}-{sequence}", std::process::id()));
        let mut builder = DirBuilder::new();
        builder.mode(0o700);
        match builder.create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("cannot create owned Cargo target: {error}")),
        }
    }
    Err("cannot allocate a unique Cargo target directory".into())
}

enum CommandResult {
    Exited(i32),
    TimedOut,
}

fn valid_crate(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
        && bytes
            .last()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
}

fn require_disk_headroom(free_bytes: u64) -> Result<(), String> {
    if free_bytes < MIN_FREE_BYTES {
        Err("CI tool install requires 2 GiB available DATA disk headroom".into())
    } else {
        Ok(())
    }
}

fn valid_version(value: &str) -> bool {
    !value.is_empty()
        && value
            .split('.')
            .all(|component| !component.is_empty() && component.bytes().all(|b| b.is_ascii_digit()))
}

fn disk_usage(path: &Path) -> Result<DiskUsage, String> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "DATA storage path contains a NUL byte")?;
    let mut value = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: statvfs writes one statvfs value to the valid out pointer; path is NUL terminated.
    let result = unsafe { libc::statvfs(path.as_ptr(), value.as_mut_ptr()) };
    if result != 0 {
        return Err(format!(
            "cannot inspect DATA storage capacity: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: statvfs returned success and initialized the output value.
    let value = unsafe { value.assume_init() };
    let block_size = u64::try_from(value.f_frsize)
        .map_err(|_| "invalid DATA filesystem fragment size")?;
    let total = u64::try_from(value.f_blocks)
        .ok()
        .and_then(|blocks| blocks.checked_mul(block_size))
        .ok_or("DATA filesystem total size overflow")?;
    let free_blocks = u64::try_from(value.f_bavail)
        .ok()
        .and_then(|blocks| blocks.checked_mul(block_size))
        .ok_or("DATA filesystem available size overflow")?;
    let free_all_blocks = u64::try_from(value.f_bfree)
        .ok()
        .and_then(|blocks| blocks.checked_mul(block_size))
        .ok_or("DATA filesystem free size overflow")?;
    Ok((total, total.saturating_sub(free_all_blocks), free_blocks))
}

fn admitted_roots() -> Result<(PathBuf, PathBuf), InstallError> {
    let storage = std::env::var_os("FABRIC_STORAGE_ROOT")
        .ok_or_else(|| InstallError::not_run("FABRIC_STORAGE_ROOT is required"))?;
    let scratch = std::env::var_os("FABRIC_SCRATCH_ROOT")
        .ok_or_else(|| InstallError::not_run("FABRIC_SCRATCH_ROOT is required"))?;
    let storage = PathBuf::from(storage)
        .canonicalize()
        .map_err(|error| InstallError::not_run(format!("cannot resolve DATA storage: {error}")))?;
    let scratch = PathBuf::from(scratch)
        .canonicalize()
        .map_err(|error| InstallError::not_run(format!("cannot resolve DATA scratch: {error}")))?;
    if !storage.is_dir() || !scratch.is_dir() || !scratch.starts_with(&storage) {
        return Err(InstallError::not_run(
            "DATA scratch must be an existing directory inside DATA storage",
        ));
    }
    Ok((storage, scratch))
}

fn open_log(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| format!("cannot open install log {}: {error}", path.display()))
}

fn run_cargo(
    cargo: &OsStr,
    arguments: &[String],
    log: File,
    timeout: Duration,
) -> Result<CommandResult, String> {
    let stderr = log
        .try_clone()
        .map_err(|error| format!("cannot duplicate install log: {error}"))?;
    let timeout_arg = format!("{}.{:03}s", timeout.as_secs(), timeout.subsec_millis());
    let started = Instant::now();
    let status = Command::new("timeout")
        .args(["--signal=KILL", timeout_arg.as_str()])
        .arg(cargo)
        .args(arguments)
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr))
        .status()
        .map_err(|error| format!("bounded Cargo invocation unavailable: {error}"))?;
    if started.elapsed() >= timeout
        && (status.code() == Some(137) || status.signal() == Some(libc::SIGKILL))
    {
        Ok(CommandResult::TimedOut)
    } else {
        Ok(CommandResult::Exited(status.code().unwrap_or(1)))
    }
}

fn write_receipt(path: &Path, receipt: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(receipt)
        .map_err(|error| format!("cannot encode install receipt: {error}"))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| format!("cannot open install receipt {}: {error}", path.display()))?;
    file.write_all(&bytes)
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|error| format!("cannot write install receipt {}: {error}", path.display()))
}

fn install(crate_name: &str, version: &str) -> Result<(), InstallError> {
    if !valid_crate(crate_name) {
        return Err(InstallError::not_run("crate name must be an ASCII Cargo name"));
    }
    if !valid_version(version) {
        return Err(InstallError::not_run(
            "version must contain nonempty numeric dot-separated components",
        ));
    }

    let (storage, scratch_root) = admitted_roots()?;
    install_at(
        crate_name,
        version,
        &storage,
        &scratch_root,
        OsStr::new("cargo"),
        INSTALL_TIMEOUT,
    )
}

fn install_at(
    crate_name: &str,
    version: &str,
    storage: &Path,
    scratch_root: &Path,
    cargo: &OsStr,
    timeout: Duration,
) -> Result<(), InstallError> {
    let before = disk_usage(&storage).map_err(InstallError::not_run)?;
    require_disk_headroom(before.free).map_err(InstallError::not_run)?;

    let output = storage.join("results/ci-tool-install");
    fs::create_dir_all(&output).map_err(|error| {
        InstallError::not_run(format!("cannot create CI tool receipt directory: {error}"))
    })?;
    let output = output.canonicalize().map_err(|error| {
        InstallError::not_run(format!("cannot resolve CI tool receipt directory: {error}"))
    })?;
    if !output.starts_with(&storage) {
        return Err(InstallError::not_run(
            "CI tool receipt directory must remain under DATA storage",
        ));
    }

    let target_path = scratch_target(&scratch_root).map_err(InstallError::not_run)?;
    let log_path = output.join(format!("{crate_name}.log"));
    let receipt_path = output.join(format!("{crate_name}.json"));
    let command = vec![
        "cargo".to_owned(),
        "install".to_owned(),
        crate_name.to_owned(),
        "--version".to_owned(),
        version.to_owned(),
        "--locked".to_owned(),
        "--target-dir".to_owned(),
        target_path.to_string_lossy().into_owned(),
    ];
    let mut receipt = json!({
        "crate": crate_name,
        "version": version,
        "target": target_path.to_string_lossy(),
        "exit": null,
        "command": command,
    });

    let operation = match open_log(&log_path) {
        Err(error) => Err(error),
        Ok(log) => match run_cargo(cargo, &command[1..], log, timeout) {
            Err(error) => Err(error),
            Ok(CommandResult::TimedOut) => Err(format!(
                "pinned Cargo tool install timed out after {} seconds; retained log: {}",
                timeout.as_secs(),
                output.display()
            )),
            Ok(CommandResult::Exited(code)) => {
                receipt["exit"] = json!(code);
                if code == 0 {
                    Ok(())
                } else {
                    Err(format!(
                        "pinned Cargo tool install failed; retained log: {}",
                        output.display()
                    ))
                }
            }
        },
    };

    let cleanup_error = fs::remove_dir_all(&target_path)
        .err()
        .filter(|error| error.kind() != std::io::ErrorKind::NotFound)
        .map(|error| format!("cannot remove Cargo target {}: {error}", target_path.display()));
    receipt["cleanup_absent"] = json!(cleanup_error.is_none() && !target_path.exists());
    let disk_after = disk_usage(&storage).map_err(InstallError::failed)?;
    receipt["disk_after"] = json!({"total":disk_after.0,"used":disk_after.1,"free":disk_after.2});

    let operation = match (operation, cleanup_error) {
        (Err(error), _) => Err(error),
        (Ok(()), Some(error)) => Err(error),
        (Ok(()), None) => Ok(()),
    };
    if let Err(error) = &operation {
        receipt["error"] = json!(error);
    }
    write_receipt(&receipt_path, &receipt).map_err(InstallError::failed)?;
    operation.map_err(InstallError::failed)
}

/// `cargo xtask install-tool <crate> <version>`.
pub fn cli(arguments: Vec<String>) -> u8 {
    if let Err(error) = crate::preflight::require_limits() {
        eprintln!("install-tool NOT RUN: {error}");
        return 2;
    }
    if arguments.len() != 2 {
        eprintln!("usage: cargo xtask install-tool <crate> <version>");
        return 2;
    }
    match install(&arguments[0], &arguments[1]) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("install-tool: {}", error.message);
            error.status
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MIN_FREE_BYTES, install_at, require_disk_headroom};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn timeout_kills_fake_cargo_and_its_child() {
        let parent = PathBuf::from(std::env::var_os("FABRIC_SCRATCH_ROOT").expect("DATA scratch"));
        let root = (0..1000_u32)
            .map(|sequence| {
                parent.join(format!(
                    "tool-install-timeout-{}-{sequence}",
                    std::process::id()
                ))
            })
            .find(|path| fs::create_dir(path).is_ok())
            .expect("unique test directory");
        let script = root.join("fake-cargo");
        let child_pid = root.join("child.pid");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nsleep 30 &\necho $! > '{}'\nwait\n",
                child_pid.display()
            ),
        )
        .expect("fake cargo script");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700))
            .expect("make fake cargo executable");
        let storage = root.join("storage");
        let scratch = storage.join("scratch");
        fs::create_dir_all(&scratch).expect("create isolated DATA roots");
        let result = install_at(
            "fake-tool",
            "1.2.3",
            &storage,
            &scratch,
            script.as_os_str(),
            Duration::from_secs(2),
        );
        assert!(result.is_err(), "timed install must fail");

        let output = storage.join("results/ci-tool-install");
        let receipt: serde_json::Value = serde_json::from_slice(
            &fs::read(output.join("fake-tool.json")).expect("timeout receipt"),
        )
        .expect("valid timeout receipt");
        let target = PathBuf::from(receipt["target"].as_str().unwrap());
        assert!(receipt["error"].as_str().unwrap().contains("timed out"));
        assert!(receipt["cleanup_absent"].as_bool().unwrap());
        assert!(!target.exists(), "timed install target must be removed");
        assert!(output.join("fake-tool.log").is_file());

        let pid: i32 = fs::read_to_string(&child_pid)
            .expect("fake child PID")
            .trim()
            .parse()
            .expect("numeric child PID");
        let stat = PathBuf::from(format!("/proc/{pid}/stat"));
        for _ in 0..40 {
            if !stat.exists() {
                break;
            }
            let contents = fs::read_to_string(&stat).unwrap_or_default();
            if process_is_zombie(&contents) {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let stopped = match fs::read_to_string(&stat) {
            Ok(contents) => process_is_zombie(&contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            Err(_) => false,
        };
        assert!(stopped, "fake cargo child remained live after timeout");
        fs::remove_dir_all(root).expect("remove test scratch");
    }

    #[test]
    fn disk_headroom_requires_the_registered_two_gibibyte_floor() {
        assert!(require_disk_headroom(MIN_FREE_BYTES - 1).is_err());
        assert!(require_disk_headroom(MIN_FREE_BYTES).is_ok());
    }

    fn process_is_zombie(contents: &str) -> bool {
        contents
            .rsplit_once(')')
            .and_then(|(_, fields)| fields.split_whitespace().next())
            == Some("Z")
    }
}
