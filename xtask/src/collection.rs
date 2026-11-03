//! Install and exercise the production local collection executables.
use crate::collection_lab::OwnedChild;
use fabric::{
    observation::{Data, Observation},
    spindle::{self, Reply, Request},
};
use std::{
    env, fs,
    io::Read,
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct Layout {
    pub home: PathBuf,
    pub runtime: PathBuf,
    pub config: PathBuf,
    pub state: PathBuf,
}
impl Layout {
    pub fn environment() -> Result<Self, String> {
        let home = PathBuf::from(env::var_os("HOME").ok_or("HOME missing")?);
        let path = |key, fallback| env::var_os(key).map(PathBuf::from).unwrap_or(fallback);
        Ok(Self {
            runtime: PathBuf::from(
                env::var_os("XDG_RUNTIME_DIR")
                    .ok_or("XDG_RUNTIME_DIR missing: use a logged-in Linux user session")?,
            ),
            config: path("XDG_CONFIG_HOME", home.join(".config")),
            state: path("XDG_STATE_HOME", home.join(".local/state")),
            home,
        })
    }
    pub fn socket(&self) -> PathBuf {
        self.runtime.join("spindle/spindle.sock")
    }
}
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn owned_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(format!("absolute path required: {}", path.display()));
    }
    for parent in path.ancestors() {
        match fs::symlink_metadata(parent) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(format!("symbolic link refused: {}", parent.display()));
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(error(e)),
        }
    }
    Ok(())
}
fn directory(path: &Path, private: bool) -> Result<(), String> {
    owned_path(path)?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(error)?;
    let m = fs::metadata(path).map_err(error)?;
    if !m.is_dir()
        || m.uid() != rustix::process::getuid().as_raw()
        || (private && m.mode() & 0o077 != 0)
    {
        return Err(format!(
            "expected owned {}directory: {} (fix permissions explicitly)",
            if private { "private " } else { "" },
            path.display()
        ));
    }
    Ok(())
}
fn check_file(path: &Path, bytes: &[u8], mode: u32) -> Result<bool, String> {
    owned_path(path)?;
    match fs::symlink_metadata(path) {
        Ok(m)
            if m.is_file()
                && m.uid() == rustix::process::getuid().as_raw()
                && m.mode() & 0o777 == mode
                && fs::read(path).map_err(error)? == bytes =>
        {
            Ok(true)
        }
        Ok(_) => Err(format!(
            "installation conflict: {}; preserve/back up and explicitly relocate the existing file before retrying",
            path.display()
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(error(e)),
    }
}
fn quoted(path: &Path, exec: bool) -> Result<String, String> {
    let text = path.to_str().ok_or("non-UTF8 service path")?;
    if text.contains(['\n', '\r', '\0']) {
        return Err("control character in service path".into());
    }
    Ok(format!(
        "\"{}\"",
        text.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', if exec { "$$" } else { "$" })
    ))
}
/// Installation never starts a service or changes existing custody/configuration.
pub fn install(layout: &Layout, binaries: &Path) -> Result<(), String> {
    directory(&layout.home, false)?;
    directory(&layout.runtime, true)?;
    let bin = layout.home.join(".local/bin");
    let config_parent = layout.config.join("fabrico11y");
    let state_parent = layout.state.join("fabrico11y");
    let runtime_parent = layout.runtime.join("spindle");
    let unit_parent = layout.config.join("systemd/user");
    for (path, private) in [
        (&bin, false),
        (&config_parent, true),
        (&state_parent, true),
        (&runtime_parent, true),
        (&unit_parent, false),
    ] {
        directory(path, private)?;
    }
    let config_path = config_parent.join("spindle.json");
    let config = spindle::Config {
        socket: layout.socket(),
        storage: state_parent.join("custody"),
        quota_bytes: 64 * 1024 * 1024,
        max_connections: 16,
        client_timeout_ms: 500,
        files: vec![],
        journal: None,
        host: None,
    };
    config.validate()?;
    let unit = format!(
        "[Unit]\nDescription=FabricO11y Spindle local observation custody\n\n[Service]\nType=simple\nExecStart={} serve {}\nExecStop={} stop {}\nRuntimeDirectory=spindle\nRuntimeDirectoryMode=0700\nUMask=0077\nNoNewPrivileges=yes\nPrivateTmp=yes\nProtectSystem=strict\nReadWritePaths={} {}\nRestart=on-failure\nRestartSec=1\nTimeoutStopSec=5\nSyslogIdentifier=spindle\n\n[Install]\nWantedBy=default.target\n",
        quoted(&bin.join("spindle"), true)?,
        quoted(&config_path, true)?,
        quoted(&bin.join("spindle"), true)?,
        quoted(&layout.socket(), true)?,
        quoted(&state_parent, false)?,
        quoted(&runtime_parent, false)?
    );
    let mut files = vec![
        (
            config_path,
            serde_json::to_vec_pretty(&config).map_err(error)?,
            0o600,
        ),
        (
            unit_parent.join("spindle.service"),
            unit.into_bytes(),
            0o644,
        ),
    ];
    for name in ["spindle", "fabric-adapter"] {
        let source = binaries.join(name);
        let m = fs::metadata(&source)
            .map_err(|e| format!("{}: {e}; build release binaries first", source.display()))?;
        if !m.is_file() || m.mode() & 0o111 == 0 {
            return Err(format!("not executable: {}", source.display()));
        }
        files.push((bin.join(name), fs::read(source).map_err(error)?, 0o755));
    }
    // Preflight all destination conflicts before writing any file.
    for (path, bytes, mode) in &files {
        check_file(path, bytes, *mode)?;
    }
    for (path, bytes, mode) in files {
        if !check_file(&path, &bytes, mode)? {
            use std::io::Write;
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(mode)
                .open(&path)
                .map_err(error)?;
            f.write_all(&bytes).map_err(error)?;
            f.set_permissions(fs::Permissions::from_mode(mode))
                .map_err(error)?;
            f.sync_all().map_err(error)?;
        }
    }
    println!(
        "{}",
        serde_json::json!({"installed":true,"socket":layout.socket(),"config":layout.config.join("fabrico11y/spindle.json"),"storage":config.storage,"activation":"systemctl --user daemon-reload && systemctl --user enable --now spindle.service"})
    );
    Ok(())
}
fn execute(command: &mut Command) -> Result<String, String> {
    let output = tempfile::tempfile().map_err(error)?;
    let diagnostics = tempfile::tempfile().map_err(error)?;
    command
        .stdin(Stdio::null())
        .stdout(output.try_clone().map_err(error)?)
        .stderr(diagnostics.try_clone().map_err(error)?)
        .process_group(0);
    let mut child = OwnedChild(command.spawn().map_err(error)?);
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.0.try_wait().map_err(error)? {
            break status;
        }
        if Instant::now() >= deadline {
            // The harness owns its adapter child; kill the isolated process group too.
            let _ = Command::new("kill")
                .args(["-KILL", "--", &format!("-{}", child.0.id())])
                .status();
            child.kill_wait();
            return Err(format!("command deadline: {command:?}"));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    use std::io::{Seek, SeekFrom};
    let read = |mut file: fs::File| -> Result<String, String> {
        if file.metadata().map_err(error)?.len() > 1024 * 1024 {
            return Err("command output exceeds smoke limit (1 MiB)".into());
        }
        file.seek(SeekFrom::Start(0)).map_err(error)?;
        let mut text = String::new();
        file.take(1024 * 1024)
            .read_to_string(&mut text)
            .map_err(error)?;
        Ok(text)
    };
    let text = read(output)?;
    if !status.success() {
        return Err(format!("{command:?}: {status}\n{}", read(diagnostics)?));
    }
    Ok(text)
}
fn export(binary: &Path, socket: &Path) -> Result<Vec<Observation>, String> {
    execute(Command::new(binary).arg("export").arg(socket))?
        .lines()
        .map(|line| serde_json::from_str(line).map_err(error))
        .collect()
}
fn harnesses(root: &Path, binaries: &Path, socket: &Path) -> Result<(), String> {
    execute(Command::new(binaries.join("examples/agent_harness")).arg(socket))?;
    execute(
        Command::new("python3")
            .arg(root.join("examples/agent_harness.py"))
            .arg(socket)
            .env("FABRIC_ADAPTER", binaries.join("fabric-adapter")),
    )?;
    Ok(())
}
fn added(before: &[Observation], after: &[Observation]) -> Result<(), String> {
    if after.len() != before.len() + 4 || after[..before.len()] != *before {
        return Err("smoke exact append/recovery mismatch (requires SDK-only idle service)".into());
    }
    let new = &after[before.len()..];
    for pair in new.as_chunks::<2>().0 {
        if !matches!(&pair[0].data, Data::ToolExecution { tool, status: fabric::observation::Outcome::Ok, .. } if tool == "multiply")
            || !matches!(
                pair[1].data,
                Data::AgentRun {
                    status: fabric::observation::Outcome::Ok,
                    ..
                }
            )
            || pair[0].identity.generation != pair[1].identity.generation
            || pair[0].identity.sequence != 1
            || pair[1].identity.sequence != 2
        {
            return Err("harness tool/run identity or outcome mismatch".into());
        }
    }
    let mut identities = std::collections::BTreeSet::new();
    for record in after {
        record.validate()?;
        if !identities.insert(serde_json::to_string(&record.identity).map_err(error)?) {
            return Err("duplicate exported identity".into());
        }
    }
    Ok(())
}
/// A supplied socket receives four observations; no stop/restart or deletion occurs.
pub fn smoke(root: &Path, binaries: &Path, socket: Option<&Path>) -> Result<(), String> {
    let spindle = binaries.join("spindle");
    if let Some(socket) = socket {
        let before = export(&spindle, socket)?;
        harnesses(root, binaries, socket)?;
        let after = export(&spindle, socket)?;
        added(&before, &after)?;
        println!(
            "{}",
            serde_json::json!({"passed":true,"mode":"existing-service","records_before":before.len(),"records_after":after.len(),"restart_checked":false})
        );
        return Ok(());
    }
    let temp = tempfile::tempdir().map_err(error)?;
    let config = crate::collection_lab::config(temp.path());
    let mut daemon = crate::collection_lab::daemon(&spindle, &config, temp.path());
    harnesses(root, binaries, &config.socket)?;
    let first = export(&spindle, &config.socket)?;
    added(&[], &first)?;
    let reply = spindle::transport::request(
        &config.socket,
        &Request::Stop,
        Instant::now() + Duration::from_secs(2),
    )?;
    if !matches!(reply, Reply::Stopped) {
        return Err("stop refused".into());
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while daemon.0.try_wait().map_err(error)?.is_none() {
        if Instant::now() >= deadline {
            return Err("daemon stop deadline".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(daemon);
    let _restarted = crate::collection_lab::daemon(&spindle, &config, temp.path());
    if export(&spindle, &config.socket)? != first {
        return Err("restart recovery differs".into());
    }
    harnesses(root, binaries, &config.socket)?;
    let second = export(&spindle, &config.socket)?;
    added(&first, &second)?;
    println!(
        "{}",
        serde_json::json!({"passed":true,"mode":"isolated","records":second.len(),"restart_checked":true})
    );
    Ok(())
}
pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let mut binaries = root.join("target/release");
    let mut socket = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--bin-dir" if index + 1 < args.len() => binaries = PathBuf::from(&args[index + 1]),
            "--socket" if index + 1 < args.len() => socket = Some(PathBuf::from(&args[index + 1])),
            _ => return Err(
                "usage: collection install|smoke [--bin-dir PATH] [--socket SOCKET (smoke only)]"
                    .into(),
            ),
        };
        index += 2;
    }
    let binaries = binaries
        .canonicalize()
        .map_err(|e| format!("binary directory: {e}; build release binaries and examples first"))?;
    match args.first().map(String::as_str) {
        Some("install") if socket.is_none() => install(&Layout::environment()?, &binaries),
        Some("smoke") => smoke(root, &binaries, socket.as_deref()),
        _ => Err(
            "usage: collection install|smoke [--bin-dir PATH] [--socket SOCKET (smoke only)]"
                .into(),
        ),
    }
}
