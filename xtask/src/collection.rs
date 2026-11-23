//! Install and exercise the production local collection executables.
use crate::collection_lab::OwnedChild;
use fabric::{
    observation::{self, Data, StoredObservation},
    spindle::{self, Reply, Request},
};
use std::{
    collections::VecDeque,
    env,
    ffi::OsString,
    fs,
    io::Read,
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Component, Path, PathBuf},
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
        let non_empty = |key| env::var_os(key).filter(|v| !v.is_empty());
        let home = PathBuf::from(non_empty("HOME").ok_or("HOME missing")?);
        let path = |key, fallback| non_empty(key).map(PathBuf::from).unwrap_or(fallback);
        Ok(Self {
            runtime: PathBuf::from(
                non_empty("XDG_RUNTIME_DIR")
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
    let uid = rustix::process::getuid().as_raw();
    let mut child_verified = false;
    for parent in path.ancestors() {
        match fs::symlink_metadata(parent) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(format!("symbolic link refused: {}", parent.display()));
            }
            Ok(m) if m.uid() != uid && m.uid() != 0 => {
                return Err(format!("untrusted parent ownership: {}", parent.display()));
            }
            Ok(m) if m.is_dir() => {
                let writable = m.mode() & 0o022 != 0;
                let safe_sticky_ancestor = child_verified && (m.mode() & 0o1000 != 0);
                if writable && !safe_sticky_ancestor {
                    return Err(format!(
                        "group/world-writable directory refused: {}",
                        parent.display()
                    ));
                }
                child_verified = true;
            }
            Ok(_) => {
                child_verified = true;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(error(e)),
        }
    }
    Ok(())
}
fn preflight_directory(path: &Path, private: bool) -> Result<(), String> {
    owned_path(path)?;
    match fs::symlink_metadata(path) {
        Ok(m) => {
            let bad_mode = if private {
                m.mode() & 0o077 != 0
            } else {
                m.mode() & 0o022 != 0
            };
            if m.file_type().is_symlink()
                || !m.is_dir()
                || m.uid() != rustix::process::getuid().as_raw()
                || bad_mode
            {
                return Err(format!(
                    "expected owned {}directory: {} (fix permissions explicitly)",
                    if private { "private " } else { "" },
                    path.display()
                ));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(error(e)),
    }
}
fn directory(path: &Path, private: bool) -> Result<(), String> {
    preflight_directory(path, private)?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(if private { 0o700 } else { 0o755 })
        .create(path)
        .map_err(error)?;
    preflight_directory(path, private)
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
        forwarding: None,
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
    for name in [
        "spindle",
        "fabric-adapter",
        "fabric-server",
        "fabric-query",
        "fabric-mcp",
    ] {
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
    let output = crate::evidence::non_tmp_unlinked_file()?;
    let diagnostics = crate::evidence::non_tmp_unlinked_file()?;
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
fn export(binary: &Path, socket: &Path) -> Result<Vec<StoredObservation>, String> {
    execute(Command::new(binary).arg("export").arg(socket))?
        .lines()
        .map(|line| observation::decode_versioned(line.as_bytes()))
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
fn added(before: &[StoredObservation], after: &[StoredObservation]) -> Result<(), String> {
    if after.len() != before.len() + 4 || after[..before.len()] != *before {
        return Err("smoke exact append/recovery mismatch (requires SDK-only idle service)".into());
    }
    let new = &after[before.len()..];
    for pair in new.as_chunks::<2>().0 {
        if !matches!(pair[0].data(), Data::ToolExecution { tool, status: fabric::observation::Outcome::Ok, .. } if tool == "multiply")
            || !matches!(
                pair[1].data(),
                Data::AgentRun {
                    status: fabric::observation::Outcome::Ok,
                    ..
                }
            )
            || pair[0].identity().generation != pair[1].identity().generation
            || pair[0].identity().sequence != 1
            || pair[1].identity().sequence != 2
        {
            return Err("harness tool/run identity or outcome mismatch".into());
        }
    }
    let mut identities = std::collections::BTreeSet::new();
    for record in after {
        if !identities.insert(serde_json::to_string(record.identity()).map_err(error)?) {
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
    let temp = crate::evidence::non_tmp_tempdir()?;
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
#[derive(Clone, Debug)]
pub struct StackInstallOptions {
    pub data_dir: PathBuf,
    pub tmp_dir: PathBuf,
    pub listen: String,
}
impl Default for StackInstallOptions {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("/data/fabrico11y"),
            tmp_dir: PathBuf::from("/data/tmp"),
            listen: "127.0.0.1:17440".into(),
        }
    }
}
fn quoted_env(key: &str, path: &Path) -> Result<String, String> {
    let text = path.to_str().ok_or("non-UTF8 service path")?;
    if text.contains(['\n', '\r', '\0']) {
        return Err("control character in service path".into());
    }
    Ok(format!(
        "\"{key}={}\"",
        text.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    ))
}
fn load_or_generate_token(path: &Path) -> Result<(Vec<u8>, String), String> {
    owned_path(path)?;
    match fs::symlink_metadata(path) {
        Ok(m) => {
            if m.file_type().is_symlink()
                || !m.is_file()
                || m.uid() != rustix::process::getuid().as_raw()
                || m.mode() & 0o777 != 0o600
                || m.len() > 256
            {
                return Err(format!(
                    "installation conflict or insecure credential: {}",
                    path.display()
                ));
            }
            let bytes = fs::read(path).map_err(error)?;
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| format!("non-UTF8 credential file: {}", path.display()))?;
            let trimmed = text.trim().to_owned();
            if !(16..=256).contains(&trimmed.len())
                || !trimmed.bytes().all(|b| b.is_ascii_graphic())
            {
                return Err(format!("invalid credential format: {}", path.display()));
            }
            Ok((bytes, trimmed))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut random = [0u8; 32];
            fs::File::open("/dev/urandom")
                .and_then(|mut f| f.read_exact(&mut random))
                .map_err(error)?;
            let token: String = random.iter().map(|b| format!("{b:02x}")).collect();
            let bytes = format!("{token}\n").into_bytes();
            Ok((bytes, token))
        }
        Err(e) => Err(error(e)),
    }
}
fn stage_executables(
    binaries: &Path,
    destination: &Path,
    names: &[&str],
) -> Result<Vec<(PathBuf, Vec<u8>, u32)>, String> {
    let mut files = Vec::with_capacity(names.len());
    for name in names {
        let source = binaries.join(name);
        let m = fs::metadata(&source)
            .map_err(|e| format!("{}: {e}; build release binaries first", source.display()))?;
        if !m.is_file() || m.mode() & 0o111 == 0 {
            return Err(format!("not executable: {}", source.display()));
        }
        files.push((
            destination.join(name),
            fs::read(&source).map_err(error)?,
            0o755,
        ));
    }
    Ok(files)
}
fn write_checked_files(files: Vec<(PathBuf, Vec<u8>, u32)>) -> Result<(), String> {
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
    Ok(())
}
fn regular_owned_file(path: &Path) -> bool {
    let uid = rustix::process::getuid().as_raw();
    fs::symlink_metadata(path)
        .is_ok_and(|m| !m.file_type().is_symlink() && m.is_file() && m.uid() == uid)
}
fn is_lock_busy(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("os error 11")
        || lower.contains("temporarily unavailable")
        || lower.contains("ewouldblock")
}
fn preflight_existing_stack_configs(
    server_cfg: &Path,
    spindle_cfg: &Path,
) -> Result<Option<(String, String)>, String> {
    owned_path(server_cfg)?;
    owned_path(spindle_cfg)?;
    let conflict = |path: &Path| {
        format!(
            "installation conflict: {}; preserve/back up and explicitly relocate the existing file before retrying",
            path.display()
        )
    };
    let uid = rustix::process::getuid().as_raw();
    let check_existing = |meta: fs::Metadata, path: &Path| -> Result<(), String> {
        if meta.file_type().is_symlink()
            || !meta.is_file()
            || meta.uid() != uid
            || meta.mode() & 0o777 != 0o600
        {
            return Err(conflict(path));
        }
        Ok(())
    };
    match (
        fs::symlink_metadata(server_cfg),
        fs::symlink_metadata(spindle_cfg),
    ) {
        (Err(e1), Err(e2))
            if e1.kind() == std::io::ErrorKind::NotFound
                && e2.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(None)
        }
        (Ok(_), Err(e)) if e.kind() == std::io::ErrorKind::NotFound => Err(conflict(server_cfg)),
        (Err(e), Ok(_)) if e.kind() == std::io::ErrorKind::NotFound => Err(conflict(spindle_cfg)),
        (Err(e), _) | (_, Err(e)) => Err(error(e)),
        (Ok(sm), Ok(pm)) => {
            check_existing(sm, server_cfg)?;
            check_existing(pm, spindle_cfg)?;
            let srv: fabric_server::Config =
                serde_json::from_slice(&fs::read(server_cfg).map_err(error)?)
                    .map_err(|_| conflict(server_cfg))?;
            let spn: spindle::Config =
                serde_json::from_slice(&fs::read(spindle_cfg).map_err(error)?)
                    .map_err(|_| conflict(spindle_cfg))?;
            if srv.nodes.len() != 1 {
                return Err(conflict(server_cfg));
            }
            let Some(ref fwd) = spn.forwarding else {
                return Err(conflict(spindle_cfg));
            };
            let origin = srv.nodes[0].origin.clone();
            let authority = fwd.authority.clone();
            if origin.is_empty()
                || origin.len() > 128
                || authority.is_empty()
                || authority.len() > 128
            {
                return Err(conflict(server_cfg));
            }
            Ok(Some((origin, authority)))
        }
    }
}
fn build_stack_configs(
    layout: &Layout,
    options: &StackInstallOptions,
    node_token_path: &Path,
    reader_token_path: &Path,
    origin: &str,
    authority: &str,
) -> Result<(fabric_server::Config, spindle::Config, Vec<u8>, Vec<u8>), String> {
    let spindle_storage = options.data_dir.join("spindle");
    let server_parent = options.data_dir.join("server");
    let server_canonical = server_parent.join("canonical");
    let server_projection = server_parent.join("projection");
    let server_config = fabric_server::Config {
        listen: options.listen.clone(),
        storage: server_canonical,
        projection: server_projection,
        quota_bytes: 256 * 1024 * 1024,
        projection_quota_bytes: 256 * 1024 * 1024,
        reader_credential_file: reader_token_path.to_path_buf(),
        nodes: vec![fabric_server::NodeGrant {
            origin: origin.to_owned(),
            credential_file: node_token_path.to_path_buf(),
        }],
        node_quota_bytes: Default::default(),
        engine_endpoint: None,
        engine_auth_token_file: None,
        allow_unauthenticated_loopback_engine: false,
        max_connections: 16,
    };
    let spindle_config = spindle::Config {
        socket: layout.socket(),
        storage: spindle_storage,
        quota_bytes: 64 * 1024 * 1024,
        forwarding: Some(spindle::ForwardConfig {
            endpoint: options.listen.clone(),
            credential_file: node_token_path.to_path_buf(),
            authority: authority.to_owned(),
            reclaim_confirmed: false,
        }),
        max_connections: 16,
        client_timeout_ms: 500,
        files: vec![],
        journal: None,
        host: None,
    };
    // Structural pre-validation before touching disk; credential file existence
    // is checked via `spindle_config.validate()` after atomic file writes.
    let mut base_check = spindle_config.clone();
    base_check.forwarding = None;
    base_check.validate()?;
    if origin.is_empty()
        || origin.len() > 128
        || authority.is_empty()
        || authority.len() > 128
        || !node_token_path.is_absolute()
    {
        return Err("invalid stack forwarding/origin metadata".into());
    }
    let server_bytes = serde_json::to_vec_pretty(&server_config).map_err(error)?;
    let spindle_bytes = serde_json::to_vec_pretty(&spindle_config).map_err(error)?;
    Ok((server_config, spindle_config, server_bytes, spindle_bytes))
}
fn build_stack_units(
    layout: &Layout,
    options: &StackInstallOptions,
    bin: &Path,
    server_config_path: &Path,
    spindle_config_path: &Path,
) -> Result<[(String, Vec<u8>); 4], String> {
    let env_tmpdir = quoted_env("TMPDIR", &options.tmp_dir)?;
    let env_data_dir = quoted_env("FABRIC_DATA_DIR", &options.data_dir)?;
    let server_unit = format!(
        "[Unit]\nDescription=FabricO11y Canonical Custody & Analytical Projection Server\nDocumentation=https://github.com/kmosoti/FabricO11y\n\n[Service]\nType=notify\nNotifyAccess=main\nSlice=fabrico11y.slice\nKillMode=control-group\nEnvironment={env_tmpdir}\nExecStart={} serve {}\nUMask=0077\nNoNewPrivileges=yes\nRestart=on-failure\nRestartSec=2\nTimeoutStopSec=10\nSyslogIdentifier=fabric-server\n\n[Install]\nWantedBy=default.target\n",
        quoted(&bin.join("fabric-server"), true)?,
        quoted(server_config_path, true)?,
    );
    let spindle_unit = format!(
        "[Unit]\nDescription=FabricO11y Spindle local observation custody\nDocumentation=https://github.com/kmosoti/FabricO11y\nAfter=fabric-server.service\nWants=fabric-server.service\n\n[Service]\nType=notify\nNotifyAccess=main\nSlice=fabrico11y.slice\nKillMode=control-group\nEnvironment={env_tmpdir}\nExecStart={} serve {}\nExecStop={} stop {}\nRuntimeDirectory=spindle\nRuntimeDirectoryMode=0700\nUMask=0077\nNoNewPrivileges=yes\nRestart=on-failure\nRestartSec=1\nTimeoutStopSec=5\nSyslogIdentifier=spindle\n\n[Install]\nWantedBy=default.target\n",
        quoted(&bin.join("spindle"), true)?,
        quoted(spindle_config_path, true)?,
        quoted(&bin.join("spindle"), true)?,
        quoted(&layout.socket(), true)?,
    );
    let daemon_unit = format!(
        "[Unit]\nDescription=FabricO11y Converged Host & Agent Telemetry Daemon\nDocumentation=https://github.com/kmosoti/FabricO11y\nAfter=fabric-server.service spindle.service\nWants=fabric-server.service spindle.service\n\n[Service]\nType=simple\nSlice=fabrico11y.slice\nKillMode=control-group\nEnvironment={env_data_dir}\nEnvironment={env_tmpdir}\nExecStart={} --data-dir {} --tmp-dir {} daemon --interval-ms 2000 --spindle-socket {}\nRuntimeDirectory=fabrico11y\nRuntimeDirectoryMode=0700\nUMask=0077\nNoNewPrivileges=yes\nRestart=on-failure\nRestartSec=3s\nTimeoutStopSec=10\nSyslogIdentifier=fabric-daemon\n\n[Install]\nWantedBy=default.target\n",
        quoted(&bin.join("fabric"), true)?,
        quoted(&options.data_dir, true)?,
        quoted(&options.tmp_dir, true)?,
        quoted(&layout.socket(), true)?,
    );
    Ok([
        (
            "fabrico11y.slice".into(),
            include_str!("../../config/fabrico11y.slice")
                .as_bytes()
                .to_vec(),
        ),
        ("fabric-server.service".into(), server_unit.into_bytes()),
        ("spindle.service".into(), spindle_unit.into_bytes()),
        ("fabric-daemon.service".into(), daemon_unit.into_bytes()),
    ])
}
fn probe_exclusive_lock_busy(lock_path: &Path) -> Result<bool, String> {
    match fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(false)
        .open(lock_path)
    {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(error(e)),
        Ok(lock_file) => match rustix::fs::flock(
            &lock_file,
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        ) {
            Ok(()) => {
                let _ = rustix::fs::flock(&lock_file, rustix::fs::FlockOperation::Unlock);
                Ok(false)
            }
            Err(e) if is_lock_busy(&e.to_string()) => Ok(true),
            Err(e) => Err(error(e)),
        },
    }
}
fn preflight_sqlite_common(
    conn: &rusqlite::Connection,
    quota: u64,
    label: &str,
) -> Result<(), String> {
    use rusqlite::OptionalExtension;

    let journal: String = conn
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .map_err(|e| format!("installation conflict: invalid {label} database: {e}"))?;
    let page_size: u64 = conn
        .pragma_query_value(None, "page_size", |r| r.get(0))
        .map_err(|e| format!("installation conflict: invalid {label} database: {e}"))?;
    let page_count: u64 = conn
        .pragma_query_value(None, "page_count", |r| r.get(0))
        .map_err(|e| format!("installation conflict: invalid {label} database: {e}"))?;
    // SQLite opens rollback-journal databases read-only with default mode "delete".
    if journal != "delete" || !(512..=65536).contains(&page_size) {
        return Err(format!(
            "installation conflict: unsupported {label} durability configuration"
        ));
    }
    let max_pages = (quota - 128 * 1024) / 2 / page_size;
    if page_count > max_pages {
        return Err(format!(
            "installation conflict: existing {label} database exceeds quota"
        ));
    }
    for (name, descriptor) in [
        (observation::REGISTRY, observation::DESCRIPTOR),
        (observation::REGISTRY_V3, observation::DESCRIPTOR_V3),
    ] {
        let existing: Option<String> = conn
            .query_row(
                "SELECT descriptor FROM registries WHERE name=?1",
                [name],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("installation conflict: invalid {label} database: {e}"))?;
        if existing.as_deref().is_some_and(|v| v != descriptor) {
            return Err(format!(
                "installation conflict: {label} registry descriptor conflict ({name})"
            ));
        }
    }
    Ok(())
}
fn preflight_stack_storage(
    spindle_storage: &Path,
    server_canonical: &Path,
    existing_configs: Option<&(String, String)>,
) -> Result<(), String> {
    let uid = rustix::process::getuid().as_raw();
    for (dir, names) in [
        (
            spindle_storage,
            ["owner.lock", "custody.sqlite", "custody.sqlite-journal"],
        ),
        (
            server_canonical,
            ["owner.lock", "canonical.sqlite", "canonical.sqlite-journal"],
        ),
    ] {
        for name in names {
            let file_path = dir.join(name);
            owned_path(&file_path)?;
            match fs::symlink_metadata(&file_path) {
                Ok(m) if m.file_type().is_symlink() || !m.is_file() || m.uid() != uid => {
                    return Err(format!(
                        "installation conflict: invalid state file {}",
                        file_path.display()
                    ));
                }
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(error(e)),
            }
        }
    }

    let spindle_db = spindle_storage.join("custody.sqlite");
    let has_spindle_db = regular_owned_file(&spindle_db);
    let canonical_db = server_canonical.join("canonical.sqlite");
    let has_canonical_db = regular_owned_file(&canonical_db);

    if !has_spindle_db
        && (spindle_storage.join("owner.lock").exists()
            || spindle_storage.join("custody.sqlite-journal").exists())
    {
        return Err(format!(
            "installation conflict: incomplete spindle custody state in {}",
            spindle_storage.display()
        ));
    }
    if !has_canonical_db
        && (server_canonical.join("owner.lock").exists()
            || server_canonical.join("canonical.sqlite-journal").exists())
    {
        return Err(format!(
            "installation conflict: incomplete canonical server state in {}",
            server_canonical.display()
        ));
    }
    if existing_configs.is_some() && (!has_spindle_db || !has_canonical_db) {
        return Err(
            "installation conflict: existing configuration without matching custody and canonical databases".into(),
        );
    }

    let ro_flags =
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if has_spindle_db {
        let busy = probe_exclusive_lock_busy(&spindle_storage.join("owner.lock"))?;
        if busy && existing_configs.is_none() {
            return Err(
                "installation conflict: active lock on unconfigured spindle storage".into(),
            );
        }
        if !busy {
            let conn = rusqlite::Connection::open_with_flags(&spindle_db, ro_flags)
                .map_err(|e| format!("installation conflict: invalid custody database: {e}"))?;
            let origin: String = conn
                .query_row(
                    "SELECT value FROM custody_state WHERE key='origin'",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| format!("installation conflict: invalid custody database: {e}"))?;
            if origin.is_empty()
                || existing_configs.is_some_and(|(ex_origin, _)| origin != *ex_origin)
            {
                return Err("installation conflict: spindle origin mismatch".into());
            }
            let lease_key: String = conn
                .query_row(
                    "SELECT value FROM custody_state WHERE key='lease_key'",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| format!("installation conflict: invalid custody database: {e}"))?;
            if lease_key.len() != 64
                || !lease_key
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            {
                return Err("installation conflict: invalid persisted custody lease key".into());
            }
            preflight_sqlite_common(&conn, 64 * 1024 * 1024, "custody")?;
        }
    }

    if has_canonical_db {
        let busy = probe_exclusive_lock_busy(&server_canonical.join("owner.lock"))?;
        if busy && existing_configs.is_none() {
            return Err(
                "installation conflict: active lock on unconfigured canonical server storage"
                    .into(),
            );
        }
        if !busy {
            let conn = rusqlite::Connection::open_with_flags(&canonical_db, ro_flags)
                .map_err(|e| format!("installation conflict: invalid canonical database: {e}"))?;
            let authority: String = conn
                .query_row("SELECT value FROM meta WHERE key='authority'", [], |r| {
                    r.get(0)
                })
                .map_err(|e| format!("installation conflict: invalid canonical database: {e}"))?;
            if authority.is_empty()
                || existing_configs.is_some_and(|(_, ex_authority)| authority != *ex_authority)
            {
                return Err("installation conflict: canonical authority mismatch".into());
            }
            let backend: String = conn
                .query_row("SELECT value FROM meta WHERE key='backend'", [], |r| {
                    r.get(0)
                })
                .map_err(|e| format!("installation conflict: invalid canonical database: {e}"))?;
            if backend != "sqlite" {
                return Err(
                    "installation conflict: unsupported canonical durability configuration".into(),
                );
            }
            preflight_sqlite_common(&conn, 256 * 1024 * 1024, "canonical")?;
        }
    }
    Ok(())
}
fn resolve_stack_identities(
    spindle_storage: &Path,
    server_canonical: &Path,
    existing_configs: Option<&(String, String)>,
) -> Result<(String, String), String> {
    let origin = match spindle::custody::Store::open(spindle_storage, 64 * 1024 * 1024) {
        Ok(store) => store.origin().to_owned(),
        Err(err)
            if is_lock_busy(&err)
                && existing_configs.is_some()
                && regular_owned_file(&spindle_storage.join("owner.lock"))
                && regular_owned_file(&spindle_storage.join("custody.sqlite")) =>
        {
            existing_configs.expect("checked Some").0.clone()
        }
        Err(err) => return Err(err),
    };
    let authority = match fabric_server::ledger::Ledger::open(server_canonical, 256 * 1024 * 1024) {
        Ok(ledger) => ledger.authority.clone(),
        Err(err)
            if is_lock_busy(&err)
                && existing_configs.is_some()
                && regular_owned_file(&server_canonical.join("owner.lock"))
                && regular_owned_file(&server_canonical.join("canonical.sqlite")) =>
        {
            existing_configs.expect("checked Some").1.clone()
        }
        Err(err) => return Err(err),
    };
    Ok((origin, authority))
}
/// Install coupled `fabric-server`, `spindle`, and `fabric-daemon` into `fabrico11y.slice`.
fn validate_host_path_no_tmp(path: &Path) -> Result<PathBuf, String> {
    fabric_core::validate_non_tmp_path(path).map_err(error)?;
    let mut resolved_parts: Vec<OsString> = Vec::new();
    let mut remaining: VecDeque<OsString> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_os_string()),
            Component::ParentDir => Some(OsString::from("..")),
            Component::RootDir | Component::CurDir | Component::Prefix(_) => None,
        })
        .collect();
    let mut hops = 0usize;
    while let Some(seg) = remaining.pop_front() {
        if seg == "." {
            continue;
        }
        if seg == ".." {
            let _ = resolved_parts.pop();
            continue;
        }
        resolved_parts.push(seg);
        let mut candidate = PathBuf::from("/");
        for part in &resolved_parts {
            candidate.push(part);
        }
        fabric_core::validate_non_tmp_path(&candidate).map_err(error)?;
        if let Ok(meta) = fs::symlink_metadata(&candidate)
            && meta.file_type().is_symlink()
        {
            hops += 1;
            if hops > 40 {
                return Err(format!(
                    "symlink loop exceeded 40 hops while checking {}",
                    path.display()
                ));
            }
            let target = fs::read_link(&candidate).map_err(error)?;
            let _ = resolved_parts.pop();
            if target.is_absolute() {
                resolved_parts.clear();
            }
            let parts: Vec<OsString> = target
                .components()
                .filter_map(|c| match c {
                    Component::Normal(s) => Some(s.to_os_string()),
                    Component::ParentDir => Some(OsString::from("..")),
                    Component::RootDir | Component::CurDir | Component::Prefix(_) => None,
                })
                .collect();
            for p in parts.into_iter().rev() {
                remaining.push_front(p);
            }
        }
    }
    let mut resolved = PathBuf::from("/");
    for part in resolved_parts {
        resolved.push(part);
    }
    fabric_core::validate_non_tmp_path(&resolved).map_err(error)
}

pub fn install_stack(
    layout: &Layout,
    binaries: &Path,
    options: &StackInstallOptions,
) -> Result<(), String> {
    for path in [
        &options.data_dir,
        &options.tmp_dir,
        &layout.home,
        &layout.runtime,
        &layout.config,
        &layout.state,
    ] {
        validate_host_path_no_tmp(path)?;
        owned_path(path)?;
    }
    let addr: std::net::SocketAddr = options
        .listen
        .parse()
        .map_err(|_| format!("invalid listen socket address: {}", options.listen))?;
    if !addr.ip().is_loopback() || addr.port() == 0 {
        return Err(format!(
            "listen address must be loopback with non-zero port: {}",
            options.listen
        ));
    }

    let bin = layout.home.join(".local/bin");
    let config_parent = layout.config.join("fabrico11y");
    let unit_parent = layout.config.join("systemd/user");
    let spindle_runtime = layout.runtime.join("spindle");
    let fabric_runtime = layout.runtime.join("fabrico11y");
    let state_dir = options.data_dir.join("state");
    let olap_dir = options.data_dir.join("olap");
    let spindle_storage = options.data_dir.join("spindle");
    let server_parent = options.data_dir.join("server");
    let server_canonical = server_parent.join("canonical");
    let server_projection = server_parent.join("projection");
    let server_config_path = config_parent.join("server.json");
    let spindle_config_path = config_parent.join("spindle.json");
    let node_token_path = config_parent.join("spindle-node.token");
    let reader_token_path = config_parent.join("fabric-reader.token");

    let mut static_files = stage_executables(
        binaries,
        &bin,
        &[
            "fabric",
            "spindle",
            "fabric-server",
            "fabric-adapter",
            "fabric-query",
            "fabric-mcp",
        ],
    )?;
    for (unit_name, unit_bytes) in build_stack_units(
        layout,
        options,
        &bin,
        &server_config_path,
        &spindle_config_path,
    )? {
        static_files.push((unit_parent.join(unit_name), unit_bytes, 0o644));
    }
    // Preflight binaries, units, credentials, and existing configs before touching disk.
    for (path, bytes, mode) in &static_files {
        check_file(path, bytes, *mode)?;
    }
    let (node_bytes, node_trimmed) = load_or_generate_token(&node_token_path)?;
    let (reader_bytes, reader_trimmed) = load_or_generate_token(&reader_token_path)?;
    if node_trimmed == reader_trimmed {
        return Err("node and reader credentials must be distinct".into());
    }
    let existing_configs =
        preflight_existing_stack_configs(&server_config_path, &spindle_config_path)?;
    if let Some((ref ex_origin, ref ex_authority)) = existing_configs {
        let (_, _, srv_bytes, spn_bytes) = build_stack_configs(
            layout,
            options,
            &node_token_path,
            &reader_token_path,
            ex_origin,
            ex_authority,
        )?;
        check_file(&server_config_path, &srv_bytes, 0o600)?;
        check_file(&spindle_config_path, &spn_bytes, 0o600)?;
    }

    let stack_dirs = [
        (&layout.home, false),
        (&layout.runtime, true),
        (&bin, false),
        (&config_parent, true),
        (&unit_parent, false),
        (&spindle_runtime, true),
        (&fabric_runtime, true),
        (&options.tmp_dir, false),
        (&options.data_dir, false),
        (&state_dir, false),
        (&olap_dir, false),
        (&spindle_storage, true),
        (&server_parent, true),
        (&server_canonical, true),
        (&server_projection, true),
    ];
    for (path, private) in stack_dirs {
        preflight_directory(path, private)?;
    }
    preflight_stack_storage(
        &spindle_storage,
        &server_canonical,
        existing_configs.as_ref(),
    )?;

    for (path, private) in stack_dirs {
        directory(path, private)?;
    }

    let (origin, authority) = resolve_stack_identities(
        &spindle_storage,
        &server_canonical,
        existing_configs.as_ref(),
    )?;
    let (_server_config, spindle_config, server_bytes, spindle_bytes) = build_stack_configs(
        layout,
        options,
        &node_token_path,
        &reader_token_path,
        &origin,
        &authority,
    )?;

    static_files.extend([
        (node_token_path, node_bytes, 0o600),
        (reader_token_path, reader_bytes, 0o600),
        (server_config_path.clone(), server_bytes, 0o600),
        (spindle_config_path.clone(), spindle_bytes, 0o600),
    ]);

    write_checked_files(static_files)?;
    spindle_config.validate()?;

    println!(
        "{}",
        serde_json::json!({
            "installed": true,
            "stack": true,
            "socket": layout.socket(),
            "listen": options.listen,
            "origin": origin,
            "authority": authority,
            "data_dir": options.data_dir,
            "server_config": server_config_path,
            "spindle_config": spindle_config_path,
            "activation": "systemctl --user daemon-reload && systemctl --user enable --now fabric-server.service spindle.service fabric-daemon.service"
        })
    );
    Ok(())
}
pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let mut binaries: Option<PathBuf> = None;
    let mut socket: Option<PathBuf> = None;
    let mut data_dir: Option<PathBuf> = None;
    let mut tmp_dir: Option<PathBuf> = None;
    let mut listen: Option<String> = None;
    let mut index = 1;
    let usage = "usage: collection install|install-stack|smoke [--bin-dir PATH] [--socket SOCKET (smoke only)] [--data-dir PATH --tmp-dir PATH --listen ADDR (install-stack only)]";
    while index < args.len() {
        let Some(value) = args.get(index + 1) else {
            return Err(usage.into());
        };
        if value.is_empty() {
            return Err(usage.into());
        }
        let set_path = |slot: &mut Option<PathBuf>| -> Result<(), String> {
            if slot.is_some() {
                return Err(usage.into());
            }
            *slot = Some(PathBuf::from(value));
            Ok(())
        };
        match args[index].as_str() {
            "--bin-dir" => set_path(&mut binaries)?,
            "--socket" => set_path(&mut socket)?,
            "--data-dir" => set_path(&mut data_dir)?,
            "--tmp-dir" => set_path(&mut tmp_dir)?,
            "--listen" => {
                if listen.is_some() {
                    return Err(usage.into());
                }
                listen = Some(value.clone());
            }
            _ => return Err(usage.into()),
        }
        index += 2;
    }
    let stack_flags = data_dir.is_some() || tmp_dir.is_some() || listen.is_some();
    let binaries = binaries
        .unwrap_or_else(|| root.join("target/release"))
        .canonicalize()
        .map_err(|e| format!("binary directory: {e}; build release binaries and examples first"))?;
    match args.first().map(String::as_str) {
        Some("install") if socket.is_none() && !stack_flags => {
            install(&Layout::environment()?, &binaries)
        }
        Some("install-stack") if socket.is_none() => install_stack(
            &Layout::environment()?,
            &binaries,
            &StackInstallOptions {
                data_dir: data_dir.unwrap_or_else(|| PathBuf::from("/data/fabrico11y")),
                tmp_dir: tmp_dir.unwrap_or_else(|| PathBuf::from("/data/tmp")),
                listen: listen.unwrap_or_else(|| "127.0.0.1:17440".into()),
            },
        ),
        Some("smoke") if !stack_flags => smoke(root, &binaries, socket.as_deref()),
        _ => Err(usage.into()),
    }
}
