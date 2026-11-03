//! Owned four-node supervisor for a fresh, externally resource-bounded trial.
//! It never adopts installed services or migrates existing collection history.
use fabric::{
    canonical::{self, CanonicalRow},
    spindle::{self, Config, ForwardConfig, Health, Reply, Request, custody::Store, transport},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        process::ExitStatusExt,
    },
    path::{Component, Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
const NODES: usize = 4;
const SHADOW: u64 = 32;
const LOG_LIMIT: u64 = 64 * 1024;
const FILE_LIMIT: u64 = 1024 * 1024;
#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Node {
    node_index: usize,
    origin: String,
    credential_file: PathBuf,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepared {
    version: u32,
    quota_bytes: u64,
    nodes: Vec<Node>,
}
struct Options(BTreeMap<String, String>);
impl Options {
    fn parse(args: &[String], allowed: &[&str]) -> Result<Self, String> {
        if !args.len().is_multiple_of(2) {
            return Err("options require --name VALUE pairs".into());
        }
        let mut values = BTreeMap::new();
        for pair in args.chunks(2) {
            if !allowed.contains(&pair[0].as_str())
                || values.insert(pair[0].clone(), pair[1].clone()).is_some()
            {
                return Err(format!("unknown or repeated option {}", pair[0]));
            }
        }
        Ok(Self(values))
    }
    fn get(&self, key: &str) -> Result<&str, String> {
        self.0
            .get(key)
            .map(String::as_str)
            .ok_or_else(|| format!("{key} required"))
    }
    fn path(&self, key: &str) -> Result<PathBuf, String> {
        let path = PathBuf::from(self.get(key)?);
        absolute_without_symlinks(&path)?;
        Ok(path)
    }
    fn number(&self, key: &str, default: u64, min: u64, max: u64) -> Result<u64, String> {
        let value = self
            .0
            .get(key)
            .map(|s| s.parse::<u64>().map_err(|_| format!("invalid {key}")))
            .transpose()?
            .unwrap_or(default);
        if !(min..=max).contains(&value) {
            return Err(format!("{key} must be in {min}..={max}"));
        }
        Ok(value)
    }
}
pub fn entry(args: &[String]) -> Result<(), String> {
    let (command, args) = args
        .split_first()
        .ok_or("usage: investigation-host prepare|run OPTIONS")?;
    match command.as_str() {
        "prepare" => prepare(args),
        "run" => run(args),
        _ => Err("unknown investigation-host command".into()),
    }
}
fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
fn uid() -> u32 {
    rustix::process::getuid().as_raw()
}
fn absolute_without_symlinks(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err("clean absolute paths required".into());
    }
    let mut prefix = PathBuf::new();
    for part in path.components() {
        prefix.push(part.as_os_str());
        match fs::symlink_metadata(&prefix) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!("symlink path refused: {}", prefix.display()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(err(error)),
        }
    }
    Ok(())
}
fn private_dir(path: &Path) -> Result<(), String> {
    absolute_without_symlinks(path)?;
    let metadata = fs::symlink_metadata(path).map_err(err)?;
    if !metadata.is_dir() || metadata.uid() != uid() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(format!(
            "private owned directory required: {}",
            path.display()
        ));
    }
    Ok(())
}
fn new_dir(path: &Path) -> Result<(), String> {
    DirBuilder::new().mode(0o700).create(path).map_err(err)
}
fn new_file(path: &Path) -> Result<File, String> {
    absolute_without_symlinks(path)?;
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("{}: {error}", path.display()))
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = new_file(path)?;
    file.write_all(bytes).map_err(err)?;
    file.sync_all().map_err(err)
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(err)?;
    if bytes.len() > FILE_LIMIT as usize {
        return Err("supervisor artifact exceeds 1 MiB".into());
    }
    write_new(path, &bytes)
}
fn read_owned(path: &Path, limit: u64, private: bool) -> Result<Vec<u8>, String> {
    absolute_without_symlinks(path)?;
    let metadata = fs::symlink_metadata(path).map_err(err)?;
    if !metadata.is_file()
        || metadata.uid() != uid()
        || metadata.len() > limit
        || (private && metadata.permissions().mode() & 0o077 != 0)
    {
        return Err(format!("bounded owned file required: {}", path.display()));
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(err)?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(err)?;
    if bytes.len() > limit as usize {
        return Err("file grew past budget".into());
    }
    Ok(bytes)
}
fn credential(path: &Path) -> Result<String, String> {
    let bytes = read_owned(path, 64, true)?;
    if bytes.len() != 64 || !bytes.iter().all(u8::is_ascii_hexdigit) {
        return Err("prepared credential must contain 64 hex bytes".into());
    }
    String::from_utf8(bytes).map_err(err)
}
fn executable(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(err)?;
    if !metadata.is_file()
        || metadata.uid() != uid()
        || metadata.permissions().mode() & 0o111 == 0
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err(format!("owned executable required: {}", path.display()));
    }
    Ok(())
}
fn node_dir(root: &Path, index: usize) -> PathBuf {
    root.join(format!("node{index}"))
}
fn prepare(args: &[String]) -> Result<(), String> {
    let options = Options::parse(args, &["--root", "--quota"])?;
    let root = options.path("--root")?;
    let quota = options.number(
        "--quota",
        64 * 1024 * 1024,
        spindle::custody::MIN_QUOTA,
        u64::MAX,
    )?;
    match fs::symlink_metadata(&root) {
        Ok(_) => {
            private_dir(&root)?;
            if fs::read_dir(&root).map_err(err)?.next().is_some() {
                return Err("prepare requires a fresh empty root".into());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => new_dir(&root)?,
        Err(error) => return Err(err(error)),
    }
    private_dir(&root)?;
    write_new(
        &root.join("prepare.claim"),
        b"fresh four-node trial preparation\n",
    )?;
    let mut nodes = Vec::new();
    for index in 0..NODES {
        let dir = node_dir(&root, index);
        new_dir(&dir)?;
        let store = Store::open(&dir.join("store"), quota)?;
        let origin = store.origin().to_owned();
        drop(store);
        let mut random = [0u8; 32];
        File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut random))
            .map_err(err)?;
        let secret: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let credential_file = dir.join("credential");
        write_new(&credential_file, secret.as_bytes())?;
        nodes.push(Node {
            node_index: index,
            origin,
            credential_file,
        });
    }
    let prepared = Prepared {
        version: 1,
        quota_bytes: quota,
        nodes,
    };
    write_json(&root.join("origins.json"), &prepared.nodes)?;
    write_json(&root.join("prepare.json"), &prepared)?;
    File::open(&root)
        .and_then(|file| file.sync_all())
        .map_err(err)?;
    println!("{}",serde_json::to_string(&json!({"kind":"four-node-investigation-prepared","root":root,"quota_per_node":quota.to_string(),"origins":prepared.nodes})).map_err(err)?);
    Ok(())
}
#[derive(Serialize)]
struct LogStats {
    observed_bytes: u64,
    retained_bytes: u64,
    truncated: bool,
}
struct OwnedChild {
    child: Child,
    label: String,
    pid: u32,
    exit: Option<ExitStatus>,
    forced: bool,
    stderr_path: PathBuf,
    log: Option<JoinHandle<Result<LogStats, String>>>,
    log_stats: Option<LogStats>,
    socket: Option<PathBuf>,
}
impl OwnedChild {
    fn spawn(
        label: String,
        program: &Path,
        args: &[String],
        stderr_path: PathBuf,
        socket: Option<PathBuf>,
    ) -> Result<Self, String> {
        let mut log_file = new_file(&stderr_path)?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(err)?;
        let pid = child.id();
        let mut stderr = child.stderr.take().ok_or("child stderr unavailable")?;
        let log = match thread::Builder::new()
            .name("investigation-stderr".into())
            .spawn(move || {
                let mut observed = 0u64;
                let mut retained = 0u64;
                let mut buffer = [0u8; 8192];
                loop {
                    let count = stderr.read(&mut buffer).map_err(err)?;
                    if count == 0 {
                        break;
                    }
                    observed = observed.saturating_add(count as u64);
                    let keep = count.min((LOG_LIMIT - retained) as usize);
                    if keep > 0 {
                        log_file.write_all(&buffer[..keep]).map_err(err)?;
                        retained += keep as u64;
                    }
                }
                log_file.sync_all().map_err(err)?;
                Ok(LogStats {
                    observed_bytes: observed,
                    retained_bytes: retained,
                    truncated: observed > retained,
                })
            }) {
            Ok(log) => log,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(err(error));
            }
        };
        Ok(Self {
            child,
            label,
            pid,
            exit: None,
            forced: false,
            stderr_path,
            log: Some(log),
            log_stats: None,
            socket,
        })
    }
    fn poll(&mut self) -> Result<Option<ExitStatus>, String> {
        if self.exit.is_none() {
            self.exit = self.child.try_wait().map_err(err)?;
        }
        Ok(self.exit)
    }
    fn finish_log(&mut self) -> Result<(), String> {
        if self.exit.is_some()
            && let Some(log) = self.log.take()
        {
            self.log_stats = Some(log.join().map_err(|_| "stderr worker panicked")??);
        }
        Ok(())
    }
    fn stop(&mut self, deadline: Instant) -> Result<(), String> {
        if self.poll()?.is_none() {
            if let Some(socket) = &self.socket {
                let _ = transport::request(
                    socket,
                    &Request::Stop,
                    deadline.min(Instant::now() + Duration::from_millis(100)),
                );
            }
            while self.poll()?.is_none() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(2));
            }
            if self.exit.is_none() {
                self.forced = true;
                self.child.kill().map_err(err)?;
                self.exit = Some(self.child.wait().map_err(err)?);
            }
        }
        self.finish_log()
    }
    fn report(&mut self) -> Value {
        let poll_error = self.poll().err();
        let log_error = self.finish_log().err();
        json!({"label":self.label,"pid":self.pid,"success":self.exit.as_ref().map(ExitStatus::success),"exit_code":self.exit.as_ref().and_then(ExitStatus::code),"signal":self.exit.as_ref().and_then(ExitStatusExt::signal),"forced_kill":self.forced,"stderr_file":self.stderr_path,"stderr":self.log_stats,"poll_error":poll_error,"stderr_error":log_error,"stdout":"discarded"})
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.exit.is_none() {
            let _ = self.stop(Instant::now() + Duration::from_millis(150));
        } else {
            let _ = self.finish_log();
        }
    }
}
#[derive(Serialize)]
struct Witness {
    node_index: usize,
    authority: String,
    origin: String,
    through: String,
    records: Vec<CanonicalRow>,
}
struct ShadowCapture {
    witness: Witness,
    path: PathBuf,
    digest: String,
}
struct ShadowJob {
    node: usize,
    worker: JoinHandle<Result<ShadowCapture, String>>,
}
struct Runtime {
    node: Node,
    config: Config,
    shadow_config: PathBuf,
    reclaim_config: PathBuf,
    loader_report: PathBuf,
    spindle: Option<OwnedChild>,
    loader: Option<OwnedChild>,
    spindle_exits: Vec<Value>,
    last_status: Option<Health>,
    last_status_at: Option<Instant>,
    shadow: Option<ShadowCapture>,
    before_restart: Option<Health>,
}
fn health(socket: &Path, deadline: Instant) -> Result<Health, String> {
    match transport::request(socket, &Request::Status, deadline)? {
        Reply::Status { status } => Ok(status),
        Reply::Error { error } => Err(error),
        _ => Err("unexpected Spindle status reply".into()),
    }
}
fn shadow_stage_error(
    node: usize,
    started: Instant,
    stage_started: Instant,
    deadline: Instant,
    stage: &str,
    error: impl std::fmt::Display,
) -> String {
    let now = Instant::now();
    format!(
        "shadow_compare node={node} stage={stage} stage_elapsed_ms={} comparison_elapsed_ms={} deadline_remaining_ms={}: {error}",
        now.saturating_duration_since(stage_started).as_millis(),
        now.saturating_duration_since(started).as_millis(),
        deadline.saturating_duration_since(now).as_millis()
    )
}
fn shadow_compare(node: Node, config: Config) -> Result<ShadowCapture, String> {
    // This budget covers 32 sequential local RPCs while ingestion continues,
    // rather than one selective query. The qualification query/drain limits
    // remain enforced independently by the frozen evaluator.
    let started = Instant::now();
    let deadline = started + Duration::from_secs(10);
    let stage_started = Instant::now();
    let before = health(&config.socket, deadline).map_err(|error| {
        shadow_stage_error(
            node.node_index,
            started,
            stage_started,
            deadline,
            "health_before",
            error,
        )
    })?;
    if before.origin != node.origin
        || before.canonical_through < SHADOW
        || before.reclaimed_through != 0
        || before.records == 0
    {
        return Err(shadow_stage_error(
            node.node_index,
            started,
            stage_started,
            deadline,
            "health_before_validation",
            "confirmed shadow prefix not retained",
        ));
    }
    let stage_started = Instant::now();
    let forwarding = config.forwarding.as_ref().ok_or_else(|| {
        shadow_stage_error(
            node.node_index,
            started,
            stage_started,
            deadline,
            "forwarding_config",
            "shadow forwarding absent",
        )
    })?;
    let secret = credential(&node.credential_file).map_err(|error| {
        shadow_stage_error(
            node.node_index,
            started,
            stage_started,
            deadline,
            "credential_load",
            error,
        )
    })?;
    let stage_started = Instant::now();
    let remote = match canonical::exchange(
        &forwarding.endpoint,
        &secret,
        &canonical::Request::Read {
            origin: node.origin.clone(),
            after: 0,
            through: SHADOW,
            limit: SHADOW as usize,
        },
        deadline,
    )
    .map_err(|error| {
        shadow_stage_error(
            node.node_index,
            started,
            stage_started,
            deadline,
            "canonical_read32",
            error,
        )
    })? {
        canonical::Response::Rows { rows } => rows,
        canonical::Response::Error { error } => {
            return Err(shadow_stage_error(
                node.node_index,
                started,
                stage_started,
                deadline,
                "canonical_read32_response",
                error,
            ));
        }
        _ => {
            return Err(shadow_stage_error(
                node.node_index,
                started,
                stage_started,
                deadline,
                "canonical_read32_response",
                "unexpected response",
            ));
        }
    };
    if remote.len() != SHADOW as usize
        || remote.iter().map(|row| row.raw.len()).sum::<usize>() > canonical::MAX_BATCH_BYTES
    {
        return Err(shadow_stage_error(
            node.node_index,
            started,
            stage_started,
            deadline,
            "canonical_read32_validation",
            "witness exceeds bounded prefix",
        ));
    }
    for (index, row) in remote.iter().enumerate() {
        let expected = index as u64 + 1;
        if row.origin != node.origin || row.source_id != expected {
            return Err(shadow_stage_error(
                node.node_index,
                started,
                stage_started,
                deadline,
                &format!("canonical_source_{expected}_validation"),
                "source prefix is not contiguous",
            ));
        }
        let stage_started = Instant::now();
        let local =
            transport::read_raw(&config.socket, expected - 1, deadline).map_err(|error| {
                shadow_stage_error(
                    node.node_index,
                    started,
                    stage_started,
                    deadline,
                    &format!("local_read_source_{expected}"),
                    error,
                )
            })?;
        if local.len() != 1 || local[0].0 != expected || local[0].1 != row.raw.as_bytes() {
            return Err(shadow_stage_error(
                node.node_index,
                started,
                stage_started,
                deadline,
                &format!("local_read_source_{expected}_validation"),
                "local and canonical bytes or source IDs differ",
            ));
        }
    }
    let stage_started = Instant::now();
    let after = health(&config.socket, deadline).map_err(|error| {
        shadow_stage_error(
            node.node_index,
            started,
            stage_started,
            deadline,
            "health_after",
            error,
        )
    })?;
    if after.origin != node.origin || after.reclaimed_through != 0 || after.records == 0 {
        return Err(shadow_stage_error(
            node.node_index,
            started,
            stage_started,
            deadline,
            "health_after_validation",
            "shadow payloads reclaimed or origin changed during comparison",
        ));
    }
    let witness = Witness {
        node_index: node.node_index,
        authority: forwarding.authority.clone(),
        origin: node.origin,
        through: SHADOW.to_string(),
        records: remote,
    };
    let stage_started = Instant::now();
    let path = config
        .storage
        .parent()
        .ok_or_else(|| {
            shadow_stage_error(
                node.node_index,
                started,
                stage_started,
                deadline,
                "witness_path",
                "node directory missing",
            )
        })?
        .join("shadow-witness.json");
    write_json(&path, &witness).map_err(|error| {
        shadow_stage_error(
            node.node_index,
            started,
            stage_started,
            deadline,
            "witness_write",
            error,
        )
    })?;
    let stage_started = Instant::now();
    let digest = super::evidence::hash(&path).map_err(|error| {
        shadow_stage_error(
            node.node_index,
            started,
            stage_started,
            deadline,
            "witness_hash",
            error,
        )
    })?;
    Ok(ShadowCapture {
        witness,
        path,
        digest,
    })
}

fn spawn_spindle(runtime: &Runtime, program: &Path, reclaim: bool) -> Result<OwnedChild, String> {
    let config = if reclaim {
        &runtime.reclaim_config
    } else {
        &runtime.shadow_config
    };
    let label = format!(
        "node{}-spindle-{}",
        runtime.node.node_index,
        if reclaim { "reclaim" } else { "shadow" }
    );
    OwnedChild::spawn(
        label.clone(),
        program,
        &["serve".into(), config.display().to_string()],
        config
            .parent()
            .ok_or("node directory missing")?
            .join(format!("{label}.stderr")),
        None,
    )
}
fn ready(nodes: &mut [Runtime], deadline: Instant) -> Result<(), String> {
    let mut seen = [false; NODES];
    while seen.iter().any(|value| !*value) {
        if Instant::now() >= deadline {
            return Err("Spindle readiness deadline".into());
        }
        for (index, node) in nodes.iter_mut().enumerate() {
            if seen[index] {
                continue;
            }
            if node
                .spindle
                .as_mut()
                .ok_or("owned Spindle missing")?
                .poll()?
                .is_some()
            {
                return Err(format!("node{index} Spindle exited before readiness"));
            }
            if let Ok(status) = health(
                &node.config.socket,
                deadline.min(Instant::now() + Duration::from_millis(100)),
            ) {
                if status.origin != node.node.origin {
                    return Err("persistent Spindle origin changed".into());
                }
                if let Some(before) = &node.before_restart
                    && status.highest_record_id < before.highest_record_id
                {
                    return Err("Spindle record allocation moved backwards on restart".into());
                }
                node.last_status = Some(status);
                node.last_status_at = Some(Instant::now());
                node.spindle.as_mut().ok_or("owned Spindle missing")?.socket =
                    Some(node.config.socket.clone());
                seen[index] = true;
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}
fn check_loaders_running(nodes: &mut [Runtime]) -> Result<(), String> {
    for node in nodes {
        if node
            .loader
            .as_mut()
            .ok_or("loader missing")?
            .poll()?
            .is_some()
        {
            return Err(format!(
                "node{} loader ended before the live shadow/reclamation transition",
                node.node.node_index
            ));
        }
    }
    Ok(())
}
fn run(args: &[String]) -> Result<(), String> {
    let options = Options::parse(
        args,
        &[
            "--root",
            "--spindle-bin",
            "--loader-bin",
            "--endpoint",
            "--authority",
            "--records",
            "--rate",
            "--seed",
            "--report",
        ],
    )?;
    let root = options.path("--root")?;
    private_dir(&root)?;
    let spindle_bin = options.path("--spindle-bin")?;
    let loader_bin = options.path("--loader-bin")?;
    executable(&spindle_bin)?;
    executable(&loader_bin)?;
    let spindle_digest = super::evidence::hash(&spindle_bin)?;
    let loader_digest = super::evidence::hash(&loader_bin)?;
    let report_path = options.path("--report")?;
    let report_parent = report_path.parent().ok_or("report parent missing")?;
    let parent_metadata = fs::symlink_metadata(report_parent).map_err(err)?;
    if !parent_metadata.is_dir()
        || parent_metadata.uid() != uid()
        || parent_metadata.permissions().mode() & 0o022 != 0
        || fs::symlink_metadata(&report_path).is_ok()
    {
        return Err(
            "fresh report path in an owned non-writable-by-others directory required".into(),
        );
    }
    let endpoint = options.get("--endpoint")?.to_owned();
    let address: std::net::SocketAddr = endpoint
        .parse()
        .map_err(|_| "endpoint must be a loopback socket address")?;
    if !address.ip().is_loopback() {
        return Err("endpoint must be a loopback tunnel".into());
    }
    let authority = options.get("--authority")?.to_owned();
    fabric::run::RunContext {
        run_id: authority.clone(),
    }
    .validate()?;
    let records = options.number("--records", 250_000, 33, 250_000)?;
    let rate = options.number("--rate", 250, 1, 1000)?;
    let seed = options
        .0
        .get("--seed")
        .map(String::as_str)
        .unwrap_or("development")
        .to_owned();
    if !matches!(seed.as_str(), "development" | "heldout") {
        return Err("seed must be development or heldout".into());
    }
    let prepared: Prepared =
        serde_json::from_slice(&read_owned(&root.join("prepare.json"), 32 * 1024, true)?)
            .map_err(err)?;
    let advertised: Vec<Node> =
        serde_json::from_slice(&read_owned(&root.join("origins.json"), 32 * 1024, true)?)
            .map_err(err)?;
    if prepared.version != 1
        || prepared.nodes.len() != NODES
        || serde_json::to_vec(&prepared.nodes).map_err(err)?
            != serde_json::to_vec(&advertised).map_err(err)?
    {
        return Err("prepared node manifest mismatch".into());
    }
    let mut unique = std::collections::BTreeSet::new();
    let mut nodes = Vec::new();
    for (index, node) in prepared.nodes.into_iter().enumerate() {
        let dir = node_dir(&root, index);
        private_dir(&dir)?;
        if node.node_index != index
            || node.credential_file != dir.join("credential")
            || !unique.insert(node.origin.clone())
        {
            return Err("prepared node paths or origins invalid".into());
        }
        let _ = credential(&node.credential_file)?;
        let store = Store::open(&dir.join("store"), prepared.quota_bytes)?;
        if store.origin() != node.origin
            || store.highest_record_id()? != 0
            || store.count()? != 0
            || store.forwarded()? != 0
            || store.reclaimed()? != 0
        {
            return Err("run requires prepared empty trial stores, never service history".into());
        }
        drop(store);
        let config = Config {
            socket: dir.join("spindle.sock"),
            storage: dir.join("store"),
            quota_bytes: prepared.quota_bytes,
            forwarding: Some(ForwardConfig {
                endpoint: endpoint.clone(),
                credential_file: node.credential_file.clone(),
                authority: authority.clone(),
                reclaim_confirmed: false,
            }),
            max_connections: 16,
            client_timeout_ms: 500,
            files: vec![],
            journal: None,
            host: None,
        };
        config.validate()?;
        match fs::symlink_metadata(&config.socket) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(err(error)),
            Ok(_) => return Err("prepared trial socket path must not already exist".into()),
        }
        nodes.push(Runtime {
            node,
            config,
            shadow_config: dir.join("config-shadow.json"),
            reclaim_config: dir.join("config-reclaim.json"),
            loader_report: dir.join("loader-report.json"),
            spindle: None,
            loader: None,
            spindle_exits: vec![],
            last_status: None,
            last_status_at: None,
            shadow: None,
            before_restart: None,
        });
    }
    write_json(
        &root.join("run.claim"),
        &json!({"supervisor_pid":std::process::id(),"authority":authority,"endpoint":endpoint,"records_per_node":records.to_string(),"rate_per_node":rate,"seed":seed}),
    )?;
    let started = Instant::now();
    let deadline = started + Duration::from_secs(records.div_ceil(rate) + 45);
    let mut shadow_jobs: Vec<ShadowJob> = Vec::new();
    let mut restart_ms = None;
    let mut live_restart_verified = false;
    let outcome = (|| {
        for node in &nodes {
            let secret = credential(&node.node.credential_file)?;
            let rpc_deadline = deadline.min(Instant::now() + Duration::from_secs(3));
            match canonical::exchange(
                &endpoint,
                &secret,
                &canonical::Request::Capabilities,
                rpc_deadline,
            )? {
                canonical::Response::Capabilities {
                    authority: actual,
                    origins,
                    ..
                } if actual == authority && origins == vec![node.node.origin.clone()] => {}
                canonical::Response::Error { error } => return Err(error),
                _ => {
                    return Err(
                        "canonical authority or node credential scope mismatch before load".into(),
                    );
                }
            }
            match canonical::exchange(
                &endpoint,
                &secret,
                &canonical::Request::Progress,
                rpc_deadline,
            )? {
                canonical::Response::Progress {
                    authority: actual,
                    frontiers,
                } if actual == authority && frontiers.values().all(|prefix| *prefix == 0) => {}
                canonical::Response::Error { error } => return Err(error),
                _ => {
                    return Err(
                        "canonical origin already has history; fresh backfill-zero trial required"
                            .into(),
                    );
                }
            }
        }
        for node in &mut nodes {
            write_json(&node.shadow_config, &node.config)?;
            let mut reclaim = node.config.clone();
            reclaim
                .forwarding
                .as_mut()
                .ok_or("forwarding missing")?
                .reclaim_confirmed = true;
            write_json(&node.reclaim_config, &reclaim)?;
            node.spindle = Some(spawn_spindle(node, &spindle_bin, false)?);
        }
        ready(
            &mut nodes,
            deadline.min(Instant::now() + Duration::from_secs(3)),
        )?;
        for node in &mut nodes {
            let arguments = vec![
                "investigation-lab".into(),
                "load".into(),
                "--socket".into(),
                node.config.socket.display().to_string(),
                "--node-index".into(),
                node.node.node_index.to_string(),
                "--records".into(),
                records.to_string(),
                "--rate".into(),
                rate.to_string(),
                "--seed".into(),
                seed.clone(),
                "--report".into(),
                node.loader_report.display().to_string(),
            ];
            node.loader = Some(OwnedChild::spawn(
                format!("node{}-loader", node.node.node_index),
                &loader_bin,
                &arguments,
                node_dir(&root, node.node.node_index).join("loader.stderr"),
                None,
            )?);
        }
        let mut comparing = [false; NODES];
        while nodes.iter().any(|node| node.shadow.is_none()) {
            if Instant::now() >= deadline {
                return Err("shared trial deadline before shadow comparison".into());
            }
            check_loaders_running(&mut nodes)?;
            for (index, node) in nodes.iter_mut().enumerate() {
                if node
                    .spindle
                    .as_mut()
                    .ok_or("Spindle missing")?
                    .poll()?
                    .is_some()
                {
                    return Err(format!(
                        "node{index} Spindle exited during shadow forwarding"
                    ));
                }
                if let Ok(status) = health(
                    &node.config.socket,
                    deadline.min(Instant::now() + Duration::from_millis(100)),
                ) {
                    if status.origin != node.node.origin || status.reclaimed_through != 0 {
                        return Err("shadow state changed origin or reclaimed payloads".into());
                    }
                    let begin = status.canonical_through >= SHADOW;
                    node.last_status = Some(status);
                    node.last_status_at = Some(Instant::now());
                    if begin && !comparing[index] {
                        let prepared = node.node.clone();
                        let config = node.config.clone();
                        shadow_jobs.push(ShadowJob {
                            node: index,
                            worker: thread::Builder::new()
                                .name("investigation-shadow".into())
                                .spawn(move || shadow_compare(prepared, config))
                                .map_err(err)?,
                        });
                        comparing[index] = true;
                    }
                }
            }
            let mut index = 0;
            while index < shadow_jobs.len() {
                if shadow_jobs[index].worker.is_finished() {
                    let job = shadow_jobs.swap_remove(index);
                    nodes[job.node].shadow = Some(
                        job.worker
                            .join()
                            .map_err(|_| "shadow comparison panicked")??,
                    );
                } else {
                    index += 1;
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
        check_loaders_running(&mut nodes)?;
        for node in &mut nodes {
            node.before_restart = Some(health(
                &node.config.socket,
                deadline.min(Instant::now() + Duration::from_millis(200)),
            )?);
        }
        let transition = Instant::now();
        let transition_deadline = deadline.min(transition + Duration::from_secs(1));
        // Issue all stop requests before waiting, then launch all replacements
        // before readiness checks. The SDK loader processes remain unchanged.
        for node in &nodes {
            let _ = transport::request(
                &node.config.socket,
                &Request::Stop,
                transition_deadline.min(Instant::now() + Duration::from_millis(100)),
            );
        }
        for node in &mut nodes {
            let mut old = node.spindle.take().ok_or("Spindle missing")?;
            old.stop(transition_deadline.min(Instant::now() + Duration::from_millis(100)))?;
            node.spindle_exits.push(old.report());
        }
        check_loaders_running(&mut nodes)?;
        for node in &mut nodes {
            node.spindle = Some(spawn_spindle(node, &spindle_bin, true)?);
        }
        let readiness = ready(&mut nodes, transition_deadline);
        restart_ms = Some(transition.elapsed().as_millis().to_string());
        readiness?;
        check_loaders_running(&mut nodes)?;
        live_restart_verified = true;
        loop {
            if Instant::now() >= deadline {
                return Err("shared loader/custody deadline".into());
            }
            let mut finished = true;
            for node in &mut nodes {
                if let Some(exit) = node.loader.as_mut().ok_or("loader missing")?.poll()? {
                    if !exit.success() {
                        let reason = read_owned(&node.loader_report, 256 * 1024, false)
                            .ok()
                            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                            .and_then(|report| {
                                report
                                    .get("error")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned)
                            })
                            .unwrap_or_else(|| format!("exit {exit}"));
                        return Err(format!(
                            "node{} loader failed: {reason}",
                            node.node.node_index
                        ));
                    }
                } else {
                    finished = false;
                }
                if node
                    .spindle
                    .as_mut()
                    .ok_or("Spindle missing")?
                    .poll()?
                    .is_some()
                {
                    return Err("Spindle exited while SDK loaders were running".into());
                }
            }
            if finished {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        for node in &nodes {
            let report: Value =
                serde_json::from_slice(&read_owned(&node.loader_report, 256 * 1024, false)?)
                    .map_err(err)?;
            if report["success"] != true
                || report["accepted"].as_str() != Some(records.to_string().as_str())
            {
                return Err("loader report did not confirm the unchanged workload".into());
            }
        }
        loop {
            if Instant::now() >= deadline {
                return Err("reclamation catch-up exceeded shared trial deadline".into());
            }
            let mut caught = true;
            for node in &mut nodes {
                let status = health(
                    &node.config.socket,
                    deadline.min(Instant::now() + Duration::from_millis(500)),
                )?;
                if status.origin != node.node.origin || status.highest_record_id != records {
                    return Err("final origin or persistent record allocation mismatch".into());
                }
                caught &= status.canonical_through == records
                    && status.reclaimed_through == records
                    && status.records == 0;
                node.last_status = Some(status);
                node.last_status_at = Some(Instant::now());
            }
            if caught {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        for node in &nodes {
            let first = transport::read_raw(
                &node.config.socket,
                0,
                deadline.min(Instant::now() + Duration::from_secs(2)),
            )?;
            let expected = &node
                .shadow
                .as_ref()
                .ok_or("shadow witness absent")?
                .witness
                .records[0];
            if first.len() != 1 || first[0].0 != 1 || first[0].1 != expected.raw.as_bytes() {
                return Err("reclaimed remote read differs from retained shadow bytes".into());
            }
        }
        Ok(())
    })();
    for job in shadow_jobs {
        let _ = job.worker.join();
    }
    let mut cleanup_errors = Vec::new();
    for node in &mut nodes {
        if let Some(loader) = &mut node.loader
            && let Err(error) = loader.stop(Instant::now() + Duration::from_millis(200))
        {
            cleanup_errors.push(error);
        }
        if let Some(spindle) = &mut node.spindle
            && let Err(error) = spindle.stop(Instant::now() + Duration::from_millis(300))
        {
            cleanup_errors.push(error);
        }
    }
    let mut final_nodes = Vec::new();
    for node in &mut nodes {
        let persisted = (|| {
            let store = Store::open(&node.config.storage, prepared.quota_bytes)?;
            let highest = store.highest_record_id()?;
            let confirmed = store.forwarded()?;
            let reclaimed = store.reclaimed()?;
            let local = store.count()?;
            Ok::<_, String>(
                json!({"origin":store.origin(),"highest_record_id":highest.to_string(),"canonical_through":confirmed.to_string(),"reclaimed_through":reclaimed.to_string(),"local_records":local.to_string(),"matches_expected_after_workload":store.origin()==node.node.origin && highest==records && confirmed==records && reclaimed==records && local==0}),
            )
        })();
        let loader_result = read_owned(&node.loader_report, 256 * 1024, false)
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(err));
        let loader = node.loader.as_mut().map(OwnedChild::report);
        let spindle = node.spindle.as_mut().map(OwnedChild::report);
        final_nodes.push(json!({"node_index":node.node.node_index,"origin":node.node.origin,"credential_file":node.node.credential_file,"loader":loader,"loader_report":loader_result.as_ref().ok(),"loader_report_error":loader_result.as_ref().err(),"spindle_shadow_exits":node.spindle_exits,"spindle_final_exit":spindle,"before_restart":node.before_restart,"last_observed_status":node.last_status,"last_observed_status_age_ms":node.last_status_at.map(|at|at.elapsed().as_millis().to_string()),"persisted":persisted.as_ref().ok(),"persisted_error":persisted.as_ref().err(),"shadow_witness":node.shadow.as_ref().map(|capture|json!({"path":capture.path,"sha256":capture.digest,"through":SHADOW.to_string()})),"next_record_id_expected":records.checked_add(1).map(|n|n.to_string()),"next_allocation_exercised":false}));
        if outcome.is_ok()
            && persisted
                .as_ref()
                .is_ok_and(|value| value["matches_expected_after_workload"] != true)
        {
            cleanup_errors.push(format!(
                "node{} persistent custody counters differ after child shutdown",
                node.node.node_index
            ));
        }
        if let Err(error) = persisted {
            cleanup_errors.push(error);
        }
    }
    let binaries_unchanged = (|| {
        Ok::<_, String>(
            super::evidence::hash(&spindle_bin)? == spindle_digest
                && super::evidence::hash(&loader_bin)? == loader_digest,
        )
    })();
    match &binaries_unchanged {
        Ok(true) => {}
        Ok(false) => cleanup_errors.push("executable artifacts changed during the trial".into()),
        Err(error) => cleanup_errors.push(format!(
            "executable artifact verification unavailable: {error}"
        )),
    }
    let success = outcome.is_ok() && cleanup_errors.is_empty();
    write_json(
        &report_path,
        &json!({"kind":"owned-four-node-investigation-host","root":root,"authority":authority,"endpoint":endpoint,"records_per_node":records.to_string(),"offered_rate_per_node":rate,"seed":seed,"spindle_bin":spindle_bin,"spindle_binary_sha256":spindle_digest,"loader_bin":loader_bin,"loader_binary_sha256":loader_digest,"binary_artifacts_unchanged":binaries_unchanged.as_ref().ok(),"elapsed_ms":started.elapsed().as_millis().to_string(),"restart_ms":restart_ms,"restart_target_ms":"1000","same_loader_processes_through_restart":live_restart_verified,"resource_enforcement":"external Olo native job: caller must enforce 2 GiB, no swap, TasksMax 256; supervisor does not set cgroups","scope":"fresh empty four-origin workload; first 32 exact local/canonical bytes before explicit reclamation; no installed-service migration or physical power-loss proof","nodes":final_nodes,"success":success,"error":outcome.as_ref().err(),"cleanup_errors":cleanup_errors}),
    )?;
    if success {
        Ok(())
    } else {
        Err(outcome.err().unwrap_or_else(|| {
            "owned child cleanup or persistent-state verification failed".into()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preparation_is_private_four_origin_and_refuses_reuse() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("trial");
        let args = vec![
            "--root".into(),
            root.display().to_string(),
            "--quota".into(),
            spindle::custody::MIN_QUOTA.to_string(),
        ];
        prepare(&args).unwrap();
        let prepared: Prepared = serde_json::from_slice(
            &read_owned(&root.join("prepare.json"), 32 * 1024, true).unwrap(),
        )
        .unwrap();
        assert_eq!(prepared.nodes.len(), NODES);
        let mut origins = std::collections::BTreeSet::new();
        let mut credentials = std::collections::BTreeSet::new();
        for node in &prepared.nodes {
            private_dir(&node_dir(&root, node.node_index)).unwrap();
            assert!(origins.insert(node.origin.clone()));
            assert!(credentials.insert(credential(&node.credential_file).unwrap()));
            let store = Store::open(
                &node_dir(&root, node.node_index).join("store"),
                prepared.quota_bytes,
            )
            .unwrap();
            assert_eq!(store.origin(), node.origin);
            assert_eq!(store.highest_record_id().unwrap(), 0);
        }
        assert!(prepare(&args).unwrap_err().contains("fresh empty root"));
    }
    #[test]
    fn public_or_symlink_trial_roots_are_refused_without_adopting_files() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let public = base.join("public");
        fs::create_dir(&public).unwrap();
        fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(prepare(&["--root".into(), public.display().to_string()]).is_err());
        assert!(fs::read_dir(&public).unwrap().next().is_none());
        let target = base.join("target");
        new_dir(&target).unwrap();
        let link = base.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(prepare(&["--root".into(), link.display().to_string()]).is_err());
        assert!(fs::read_dir(&target).unwrap().next().is_none());
    }
}
