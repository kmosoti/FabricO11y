//! Explicit replayable sources. Checkpoints move in the custody transaction only.
use super::custody::{Checkpoint, Store};
use crate::observation::{self, Data, Identity, Level, MetricType, Observation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::PathBuf,
    time::{Duration, Instant},
};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    pub id: String,
    pub path: PathBuf,
    pub format: FileFormat,
    #[serde(default)]
    pub replay: bool,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileFormat {
    Text,
    Jsonl,
}
impl FileConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() || self.id.len() > 120 || !self.path.is_absolute() {
            Err("invalid file source".into())
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SourceStatus {
    pub source: String,
    pub state: String,
    pub lag_bytes: u64,
    pub errors: u64,
    pub last_success_ns: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct Position {
    dev: u64,
    ino: u64,
    offset: u64,
    epoch: u64,
    tail: String,
    skipping: bool,
}
fn tail(file: &mut File, offset: u64) -> Result<String, String> {
    let len = offset.min(32);
    file.seek(SeekFrom::Start(offset - len)).map_err(err)?;
    let mut b = vec![0; len as usize];
    file.read_exact(&mut b).map_err(err)?;
    Ok(format!("{:x}", Sha256::digest(b)))
}
pub struct FileSource {
    config: FileConfig,
    file: Option<File>,
    position: Option<Position>,
    pub status: SourceStatus,
}
fn envelope(producer: String, generation: String, sequence: u64, data: Data) -> Observation {
    Observation {
        version: 2,
        registry: observation::REGISTRY.into(),
        identity: Identity {
            producer,
            generation,
            sequence,
        },
        timestamp_ns: observation::now_ns(),
        resource: "localhost".into(),
        data,
    }
}
fn record(store: &mut Store, o: &Observation, cp: Option<&Checkpoint>) -> Result<(), String> {
    let bytes = serde_json::to_vec(o).map_err(err)?;
    store.submit(&bytes, cp).map(|_| ())
}
impl FileSource {
    pub fn new(config: FileConfig) -> Self {
        let source = format!("file:{}", config.id);
        Self {
            config,
            file: None,
            position: None,
            status: SourceStatus {
                source,
                ..Default::default()
            },
        }
    }
    fn commit(&mut self, store: &mut Store, mut p: Position, data: Data) -> Result<(), String> {
        p.tail = tail(self.file.as_mut().ok_or("file closed")?, p.offset)?;
        let mut o = envelope(
            self.status.source.clone(),
            format!("{}:{}:{}", p.dev, p.ino, p.epoch),
            p.offset.saturating_add(1),
            data,
        );
        if serde_json::to_vec(&o)
            .map_err(err)
            .and_then(|b| observation::decode(&b).map(|_| ()))
            .is_err()
        {
            o.data = Data::Gap {
                source: self.status.source.clone(),
                reason: "source record exceeds observation wire limits".into(),
            };
        }
        record(
            store,
            &o,
            Some(&Checkpoint {
                source: self.status.source.clone(),
                position: serde_json::to_string(&p).map_err(err)?,
            }),
        )?;
        self.position = Some(p);
        self.status.last_success_ns = observation::now_ns();
        Ok(())
    }
    pub fn poll(&mut self, store: &mut Store) {
        if let Err(e) = self.poll_inner(store) {
            if self.position.is_none() {
                self.file = None;
            }
            self.status.state = e;
            self.status.errors = self.status.errors.saturating_add(1)
        }
    }
    fn poll_inner(&mut self, store: &mut Store) -> Result<(), String> {
        if self.file.is_none() {
            let file = open_source(&self.config.path)?;
            let meta = file.metadata().map_err(err)?;
            if !meta.is_file() {
                return Err("source is not a regular file".into());
            }
            let saved = store
                .checkpoint(&self.status.source)?
                .map(|s| serde_json::from_str::<Position>(&s).map_err(err))
                .transpose()?;
            self.file = Some(file);
            match saved {
                Some(p) if p.dev == meta.dev() && p.ino == meta.ino() => self.position = Some(p),
                Some(p) => {
                    let next = Position {
                        dev: meta.dev(),
                        ino: meta.ino(),
                        epoch: p.epoch.saturating_add(1),
                        ..Default::default()
                    };
                    self.commit(
                        store,
                        next,
                        Data::Gap {
                            source: self.status.source.clone(),
                            reason:
                                "file identity changed while stopped; previous inode unavailable"
                                    .into(),
                        },
                    )?;
                }
                None => {
                    let p = Position {
                        dev: meta.dev(),
                        ino: meta.ino(),
                        offset: if self.config.replay { 0 } else { meta.len() },
                        ..Default::default()
                    };
                    self.commit(
                        store,
                        p,
                        Data::Gap {
                            source: self.status.source.clone(),
                            reason: if self.config.replay {
                                "file replay begins"
                            } else {
                                "file attached at current end; earlier bytes excluded"
                            }
                            .into(),
                        },
                    )?;
                }
            }
        }
        for _ in 0..8 {
            let p = self.position.clone().ok_or("checkpoint unavailable")?;
            let file = self.file.as_mut().ok_or("file unavailable")?;
            let meta = file.metadata().map_err(err)?;
            self.status.lag_bytes = meta.len().saturating_sub(p.offset);
            if meta.len() < p.offset || (!p.tail.is_empty() && tail(file, p.offset)? != p.tail) {
                self.commit(
                    store,
                    Position {
                        dev: p.dev,
                        ino: p.ino,
                        epoch: p.epoch.saturating_add(1),
                        ..Default::default()
                    },
                    Data::Gap {
                        source: self.status.source.clone(),
                        reason: "file truncated or modified behind committed offset".into(),
                    },
                )?;
                continue;
            }
            file.seek(SeekFrom::Start(p.offset)).map_err(err)?;
            let mut bytes = vec![0; observation::MAX_FRAME + 1];
            let n = file.read(&mut bytes).map_err(err)?;
            bytes.truncate(n);
            let path_meta = fs::metadata(&self.config.path).ok();
            let rotated = path_meta
                .as_ref()
                .is_some_and(|m| m.dev() != p.dev || m.ino() != p.ino);
            if n == 0 {
                if rotated {
                    let file = open_source(&self.config.path)?;
                    let m = file.metadata().map_err(err)?;
                    let old = self.file.replace(file);
                    if let Err(e) = self.commit(
                        store,
                        Position {
                            dev: m.dev(),
                            ino: m.ino(),
                            epoch: p.epoch.saturating_add(1),
                            ..Default::default()
                        },
                        Data::Gap {
                            source: self.status.source.clone(),
                            reason: "rename rotation: previous descriptor drained".into(),
                        },
                    ) {
                        self.file = old;
                        return Err(e);
                    }
                    continue;
                }
                self.status.state = "caught_up".into();
                return Ok(());
            }
            let end = bytes.iter().position(|&b| b == b'\n').map(|x| x + 1);
            if end.is_none() && n <= observation::MAX_FRAME && !rotated {
                self.status.state = "partial_line".into();
                return Ok(());
            }
            let consume = end.unwrap_or(n);
            let mut next = p.clone();
            next.offset = next
                .offset
                .checked_add(consume as u64)
                .ok_or("file offset overflow")?;
            next.skipping = end.is_none() && !rotated;
            let data = if p.skipping || end.is_none() || consume > 32768 {
                Data::Gap {
                    source: self.status.source.clone(),
                    reason: if rotated && end.is_none() {
                        "rotated unterminated line rejected"
                    } else {
                        "oversized line or continuation rejected"
                    }
                    .into(),
                }
            } else {
                let line = &bytes[..consume - 1];
                match self.config.format {
                    FileFormat::Text => match String::from_utf8(line.to_vec()) {
                        Ok(message) if !message.is_empty() => Data::Log {
                            level: Level::Info,
                            message,
                        },
                        _ => Data::Gap {
                            source: self.status.source.clone(),
                            reason: "empty or invalid UTF-8 line rejected".into(),
                        },
                    },
                    FileFormat::Jsonl => match observation::check_json(line, 32768).and_then(|()| {
                        serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(line)
                            .map_err(err)
                    }) {
                        Ok(fields) => {
                            let data = Data::JsonLog { fields };
                            let probe = envelope(
                                self.status.source.clone(),
                                "probe".into(),
                                1,
                                data.clone(),
                            );
                            if probe.validate().is_ok() {
                                data
                            } else {
                                Data::Gap {
                                    source: self.status.source.clone(),
                                    reason: "JSONL fields exceed typed limits".into(),
                                }
                            }
                        }
                        Err(e) => Data::Gap {
                            source: self.status.source.clone(),
                            reason: format!("JSONL rejected: {e}"),
                        },
                    },
                }
            };
            self.commit(store, next, data)?;
        }
        self.status.state = "collecting".into();
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalConfig {
    pub unit: String,
    #[serde(default)]
    pub system: bool,
    #[serde(default)]
    pub replay: bool,
    #[serde(default)]
    pub include_self: bool,
}
impl JournalConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.unit.is_empty() || self.unit.len() > 110 || self.unit.starts_with('-') {
            Err("invalid journal unit".into())
        } else if !self.include_self && self.unit == "spindle.service" {
            Err("self journal excluded".into())
        } else {
            Ok(())
        }
    }
}
/// Child pipes are nonblocking, total output and wall time capped, and always reaped.
fn journal_command(args: &[String]) -> Result<Vec<u8>, String> {
    use rustix::fs::{OFlags, fcntl_setfl};
    use std::process::{Command, Stdio};
    let mut child = Command::new("journalctl")
        .args(args)
        .env("SYSTEMD_COLORS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(err)?;
    let mut stdout = child.stdout.take().ok_or("missing stdout")?;
    let mut stderr = child.stderr.take().ok_or("missing stderr")?;
    if let Err(e) =
        fcntl_setfl(&stdout, OFlags::NONBLOCK).and_then(|()| fcntl_setfl(&stderr, OFlags::NONBLOCK))
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(err(e));
    }
    let deadline = Instant::now() + Duration::from_millis(250);
    let mut out = Vec::new();
    let mut diag = Vec::new();
    let mut chunk = [0; 8192];
    let (mut out_eof, mut err_eof) = (false, false);
    let mut exit = None;
    let result = loop {
        for (pipe, buf, eof) in [
            (&mut stdout as &mut dyn Read, &mut out, &mut out_eof),
            (&mut stderr as &mut dyn Read, &mut diag, &mut err_eof),
        ] {
            loop {
                match pipe.read(&mut chunk) {
                    Ok(0) => {
                        *eof = true;
                        break;
                    }
                    Ok(n) => {
                        if buf.len() + n > 262144 {
                            let _ = child.kill();
                            let _ = child.wait();
                            return Err("journal output limit".into());
                        }
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(err(e));
                    }
                }
            }
        }
        if exit.is_none() {
            match child.try_wait() {
                Ok(status) => exit = status,
                Err(e) => break Err(err(e)),
            }
        }
        if let Some(status) = exit
            && out_eof
            && err_eof
        {
            if status.success() && diag.is_empty() {
                break Ok(out);
            } else {
                break Err(format!(
                    "journal unavailable: {status}: {}",
                    String::from_utf8_lossy(&diag)
                ));
            }
        }
        if Instant::now() >= deadline {
            break Err("journal deadline".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let _ = child.kill();
    let _ = child.wait();
    result
}
pub struct JournalSource {
    config: JournalConfig,
    pub status: SourceStatus,
    next_poll: Instant,
}
impl JournalSource {
    pub fn new(config: JournalConfig) -> Self {
        Self {
            status: SourceStatus {
                source: format!(
                    "journal:{}:{}",
                    if config.system { "system" } else { "user" },
                    config.unit
                ),
                ..Default::default()
            },
            config,
            next_poll: Instant::now(),
        }
    }
    pub fn poll(&mut self, store: &mut Store) {
        if Instant::now() < self.next_poll {
            return;
        }
        self.next_poll = Instant::now() + Duration::from_secs(1);
        if let Err(e) = self.inner(store) {
            self.status.state = e;
            self.status.errors = self.status.errors.saturating_add(1)
        }
    }
    fn inner(&mut self, store: &mut Store) -> Result<(), String> {
        let saved = store.checkpoint(&self.status.source)?;
        let attached = saved.as_deref().is_some_and(|s| s.starts_with("@since:"));
        let mut args=vec!["--no-pager".into(),"--output=json".into(),"--output-fields=__CURSOR,__REALTIME_TIMESTAMP,MESSAGE,_SYSTEMD_USER_UNIT,_SYSTEMD_UNIT,SYSLOG_IDENTIFIER".into()];
        if self.config.system {
            args.push(format!("--unit={}", self.config.unit))
        } else {
            args.push("--user".into());
            args.push(format!("--user-unit={}", self.config.unit))
        }
        if !self.config.include_self {
            args.push("--exclude-identifier=spindle".into())
        }
        if attached {
            args.push(format!(
                "--since={}",
                saved
                    .as_deref()
                    .unwrap_or_default()
                    .trim_start_matches("@since:")
            ));
            args.push("--lines=+8".into())
        } else if let Some(cursor) = &saved {
            args.push(format!("--cursor={cursor}"));
            args.push("--lines=+9".into())
        } else {
            args.push(
                if self.config.replay {
                    "--lines=+8"
                } else {
                    "--lines=1"
                }
                .into(),
            )
        }
        let attach_ns = observation::now_ns();
        let output = journal_command(&args)?;
        self.process_output(store, saved, output, attach_ns)
    }
    fn process_output(
        &mut self,
        store: &mut Store,
        saved: Option<String>,
        output: Vec<u8>,
        attach_ns: u64,
    ) -> Result<(), String> {
        let attached = saved.as_deref().is_some_and(|s| s.starts_with("@since:"));
        let lines: Vec<_> = output
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .collect();
        if lines.is_empty() {
            if saved.is_none() && !self.config.replay {
                let now = attach_ns;
                let cp = Checkpoint {
                    source: self.status.source.clone(),
                    position: format!("@since:@{}.{:06}", now / 1000000000, now / 1000 % 1000000),
                };
                record(
                    store,
                    &envelope(
                        self.status.source.clone(),
                        format!("attach-{now}"),
                        1,
                        Data::Gap {
                            source: self.status.source.clone(),
                            reason: "empty journal attached at current time".into(),
                        },
                    ),
                    Some(&cp),
                )?;
            }
            self.status.state = if saved.is_some() {
                "cursor unavailable: no accessible entries"
            } else {
                "no accessible journal entries"
            }
            .into();
            return Ok(());
        }
        for (index, line) in lines.into_iter().enumerate() {
            observation::check_json(line, observation::MAX_FRAME)?;
            let entry: serde_json::Value = serde_json::from_slice(line).map_err(err)?;
            let cursor = entry["__CURSOR"]
                .as_str()
                .filter(|s| s.len() <= 4096)
                .ok_or("journal cursor missing")?;
            if index == 0 && saved.as_deref() == Some(cursor) {
                continue;
            }
            let excluded = !self.config.include_self
                && (entry["_SYSTEMD_USER_UNIT"] == "spindle.service"
                    || entry["_SYSTEMD_UNIT"] == "spindle.service"
                    || entry["SYSLOG_IDENTIFIER"] == "spindle");
            let data = if index == 0 && saved.is_some() && !attached {
                Data::Gap {
                    source: "journal".into(),
                    reason:
                        "saved cursor absent or vacuumed; first available entry explicitly skipped"
                            .into(),
                }
            } else if saved.is_none() && !self.config.replay {
                Data::Gap {
                    source: "journal".into(),
                    reason: "journal attached at current end; earlier entries excluded".into(),
                }
            } else if excluded {
                Data::Gap {
                    source: "journal".into(),
                    reason: "self journal entry excluded".into(),
                }
            } else {
                match entry["MESSAGE"].as_str() {
                    Some(message) if !message.is_empty() && message.len() <= 32768 => Data::Log {
                        level: Level::Info,
                        message: message.into(),
                    },
                    _ => Data::Gap {
                        source: "journal".into(),
                        reason: "journal message binary, empty or oversized".into(),
                    },
                }
            };
            let mut o = envelope(
                self.status.source.clone(),
                format!("{:x}", Sha256::digest(cursor.as_bytes())),
                1,
                data,
            );
            if let Some(timestamp) = entry["__REALTIME_TIMESTAMP"]
                .as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .and_then(|n| n.checked_mul(1000))
            {
                o.timestamp_ns = timestamp;
            }
            record(
                store,
                &o,
                Some(&Checkpoint {
                    source: self.status.source.clone(),
                    position: cursor.into(),
                }),
            )?;
            self.status.last_success_ns = observation::now_ns();
        }
        self.status.state = "collecting".into();
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostConfig {
    pub interval_ms: u64,
    pub filesystems: Vec<PathBuf>,
}
impl HostConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.interval_ms < 1000
            || self.interval_ms > 86400000
            || self.filesystems.len() > 16
            || self.filesystems.iter().any(|p| !p.is_absolute())
        {
            Err("invalid host sampling limits".into())
        } else {
            Ok(())
        }
    }
}
pub struct HostSource {
    config: HostConfig,
    next: Instant,
    previous: BTreeMap<String, f64>,
    boot: String,
    generation: String,
    sequence: u64,
    pub missed: u64,
    pending_missed: u64,
    pending: std::collections::VecDeque<Sample>,
    pub status: SourceStatus,
}
impl HostSource {
    pub fn new(config: HostConfig) -> Self {
        Self {
            config,
            next: Instant::now(),
            previous: BTreeMap::new(),
            boot: String::new(),
            generation: fs::read_to_string("/proc/sys/kernel/random/uuid")
                .unwrap_or_else(|_| observation::now_ns().to_string())
                .trim()
                .into(),
            sequence: 0,
            missed: 0,
            pending_missed: 0,
            pending: std::collections::VecDeque::new(),
            status: SourceStatus {
                source: "host".into(),
                ..Default::default()
            },
        }
    }
    pub fn poll(&mut self, store: &mut Store) {
        let due = Instant::now() >= self.next;
        if !due && self.pending.is_empty() {
            return;
        }
        if due {
            if !self.pending.is_empty() {
                self.pending.clear();
                self.missed = self.missed.saturating_add(1);
                self.pending_missed = self.pending_missed.saturating_add(1)
            }
            self.next = Instant::now() + Duration::from_millis(self.config.interval_ms);
        }
        if let Err(e) = self.inner(store, due) {
            self.pending.clear();
            self.missed = self.missed.saturating_add(1);
            self.pending_missed = self.pending_missed.saturating_add(1);
            self.status.errors = self.status.errors.saturating_add(1);
            self.status.state = e;
        }
    }
    fn emit(&mut self, store: &mut Store, data: Data) -> Result<(), String> {
        self.sequence = self.sequence.checked_add(1).ok_or("sequence exhausted")?;
        record(
            store,
            &envelope("host".into(), self.generation.clone(), self.sequence, data),
            None,
        )
    }
    fn inner(&mut self, store: &mut Store, due: bool) -> Result<(), String> {
        if self.pending_missed > 0 {
            self.emit(
                store,
                Data::Gap {
                    source: "host".into(),
                    reason: format!(
                        "{} sample intervals missed; no backfill",
                        self.pending_missed
                    ),
                },
            )?;
            self.pending_missed = 0;
        }
        if due {
            let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id").map_err(err)?;
            if boot != self.boot {
                self.previous.clear();
                self.boot = boot;
                self.generation = format!(
                    "{}:{}",
                    self.generation.split(':').next().unwrap_or("host"),
                    self.boot.trim()
                )
            }
            self.pending = host_samples(&self.config)?.into();
        }
        for _ in 0..4 {
            let Some((name, unit, value, metric_type)) = self.pending.pop_front() else {
                break;
            };
            let reset = metric_type == MetricType::Counter
                && self.previous.get(&name).is_none_or(|old| value < *old);
            self.emit(
                store,
                Data::Metric {
                    name: name.clone(),
                    unit,
                    value,
                    metric_type,
                    reset,
                },
            )?;
            self.previous.insert(name, value);
        }
        self.status.state = "sampling".into();
        self.status.last_success_ns = observation::now_ns();
        Ok(())
    }
}
type Sample = (String, String, f64, MetricType);
fn bounded_file(path: &str) -> Result<String, String> {
    let mut b = Vec::new();
    File::open(path)
        .map_err(err)?
        .take(262145)
        .read_to_end(&mut b)
        .map_err(err)?;
    if b.len() > 262144 {
        return Err(format!("{path} exceeds read limit"));
    }
    String::from_utf8(b).map_err(err)
}
fn host_samples(config: &HostConfig) -> Result<Vec<Sample>, String> {
    let mut out = Vec::new();
    let stat = bounded_file("/proc/stat")?;
    let cpu = stat.lines().next().ok_or("CPU unavailable")?;
    let nums: Vec<u64> = cpu
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(|s| s.parse::<u64>().map_err(err))
        .collect::<Result<_, _>>()?;
    out.push((
        "host.cpu.total".into(),
        "tick".into(),
        nums.iter().map(|n| *n as f64).sum(),
        MetricType::Counter,
    ));
    if let Some(idle) = nums.get(3) {
        out.push((
            "host.cpu.idle".into(),
            "tick".into(),
            *idle as f64,
            MetricType::Counter,
        ));
    }
    for line in bounded_file("/proc/meminfo")?.lines() {
        let mut fields = line.split_whitespace();
        let key = fields.next().unwrap_or("");
        if matches!(key, "MemTotal:" | "MemAvailable:") {
            let value = fields
                .next()
                .ok_or("memory unavailable")?
                .parse::<u64>()
                .map_err(err)?;
            out.push((
                format!("host.memory.{}", key.trim_end_matches(':')),
                "By".into(),
                value as f64 * 1024.,
                MetricType::Gauge,
            ));
        }
    }
    for line in bounded_file("/proc/net/dev")?.lines().skip(2).take(64) {
        if let Some((interface, values)) = line.split_once(':') {
            let nums: Vec<&str> = values.split_whitespace().collect();
            for (index, label) in [(0, "received"), (8, "sent")] {
                if let Some(value) = nums.get(index) {
                    out.push((
                        format!("host.network.{}.{}", interface.trim(), label),
                        "By".into(),
                        value.parse::<u64>().map_err(err)? as f64,
                        MetricType::Counter,
                    ));
                }
            }
        }
    }
    for line in bounded_file("/proc/diskstats")?.lines().take(64) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() >= 14 {
            for (index, label) in [(5, "read"), (9, "written")] {
                out.push((
                    format!("host.disk.{}.{}", f[2], label),
                    "By".into(),
                    f[index].parse::<u64>().map_err(err)? as f64 * 512.,
                    MetricType::Counter,
                ));
            }
        }
    }
    for path in &config.filesystems {
        let stat = rustix::fs::statvfs(path).map_err(err)?;
        for (value, label) in [(stat.f_blocks, "capacity"), (stat.f_bavail, "available")] {
            out.push((
                format!("host.filesystem.{}.{}", path.display(), label),
                "By".into(),
                value as f64 * stat.f_frsize as f64,
                MetricType::Gauge,
            ));
        }
    }
    Ok(out)
}
pub fn process_resident_bytes() -> u64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|n| n.parse::<u64>().ok())
        })
        .unwrap_or(0)
        * 1024
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn open_source(path: &std::path::Path) -> Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::NOFOLLOW).bits() as i32)
        .open(path)
        .map_err(err)?;
    if !file.metadata().map_err(err)?.is_file() {
        return Err("source must be a regular file (symlinks excluded)".into());
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn journal(replay: bool) -> JournalSource {
        JournalSource::new(JournalConfig {
            unit: "fixture.service".into(),
            system: false,
            replay,
            include_self: false,
        })
    }
    fn output() -> Vec<u8> {
        b"{\"__CURSOR\":\"cursor-a\",\"__REALTIME_TIMESTAMP\":\"123\",\"MESSAGE\":\"first\"}\n{\"__CURSOR\":\"cursor-b\",\"MESSAGE\":\"second\"}\n".to_vec()
    }
    #[test]
    fn journal_replay_restart_missing_cursor_and_self_exclusion() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(&temp.path().join("store"), 2 * 1024 * 1024).unwrap();
        let mut source = journal(true);
        source
            .process_output(&mut store, None, output(), 1)
            .unwrap();
        assert_eq!(store.count().unwrap(), 2);
        assert_eq!(store.read(0, 1).unwrap()[0].1.timestamp_ns, 123000);
        let saved = store.checkpoint(&source.status.source).unwrap();
        let mut restarted = journal(true);
        restarted
            .process_output(
                &mut store,
                saved,
                b"{\"__CURSOR\":\"cursor-b\",\"MESSAGE\":\"second\"}\n".to_vec(),
                1,
            )
            .unwrap();
        assert_eq!(store.count().unwrap(), 2);
        restarted.process_output(&mut store,Some("vacuumed".into()),b"{\"__CURSOR\":\"cursor-c\",\"MESSAGE\":\"third\"}\n{\"__CURSOR\":\"cursor-d\",\"MESSAGE\":\"self\",\"SYSLOG_IDENTIFIER\":\"spindle\"}\n".to_vec(),1).unwrap();
        assert_eq!(store.count().unwrap(), 4);
        for (_, o) in store.read(2, 10).unwrap() {
            assert!(matches!(o.data, Data::Gap { .. }))
        }
        assert_eq!(
            store.checkpoint(&source.status.source).unwrap().as_deref(),
            Some("cursor-d")
        );
    }
    #[test]
    fn empty_journal_boundary_preserves_first_later_event() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(&temp.path().join("store"), 2 * 1024 * 1024).unwrap();
        let mut source = journal(false);
        source
            .process_output(&mut store, None, Vec::new(), 1000000000)
            .unwrap();
        let cp = store.checkpoint(&source.status.source).unwrap();
        assert_eq!(cp.as_deref(), Some("@since:@1.000000"));
        source
            .process_output(&mut store, cp, output(), 2000000000)
            .unwrap();
        assert_eq!(store.count().unwrap(), 3);
        assert!(matches!(
            store.read(1, 1).unwrap()[0].1.data,
            Data::Log { .. }
        ));
    }
    #[test]
    fn host_boot_and_counter_resets_and_failed_intervals_are_explicit() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(&temp.path().join("store"), 4 * 1024 * 1024).unwrap();
        let mut source = HostSource::new(HostConfig {
            interval_ms: 1000,
            filesystems: vec![],
        });
        source.inner(&mut store, true).unwrap();
        for (_, o) in store.read(0, 10).unwrap() {
            if let Data::Metric {
                metric_type: MetricType::Counter,
                reset,
                ..
            } = o.data
            {
                assert!(reset);
                assert!(o.identity.generation.contains(source.boot.trim()));
            }
        }
        source.pending.clear();
        source.previous.insert("counter".into(), 10.);
        source
            .pending
            .push_back(("counter".into(), "1".into(), 2., MetricType::Counter));
        source.inner(&mut store, false).unwrap();
        let records = store.read(0, 100).unwrap();
        assert!(matches!(
            records.last().unwrap().1.data,
            Data::Metric { reset: true, .. }
        ));
        let mut full = Store::open(&temp.path().join("full"), 1024 * 1024).unwrap();
        for seq in 1..100 {
            let o = envelope(
                "fill".into(),
                "g".into(),
                seq,
                Data::Log {
                    level: Level::Info,
                    message: "x".repeat(32000),
                },
            );
            if record(&mut full, &o, None).is_err() {
                break;
            }
        }
        source.next = Instant::now();
        source.poll(&mut full);
        assert!(source.missed > 0);
        source.next = Instant::now();
        source.poll(&mut store);
        assert!(
            store
                .read(0, 100)
                .unwrap()
                .iter()
                .any(|(_, o)| matches!(&o.data,Data::Gap{reason,..}if reason.contains("missed")))
        );
    }
}
