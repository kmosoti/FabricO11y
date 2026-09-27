//! One-process native collector. Each cycle owns a candidate until journal commit.

use crate::alpha::host::{self, Kind, Paths, Value};
use crate::alpha::journal::{Batch, Cursor, Journal, MAX_GAP_BYTES, MAX_GAPS_PER_BATCH};
use crate::alpha::log_source;
use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
};
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    AggregationTemporality, Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum,
    metric, number_data_point,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const MAX_LOGS: usize = 16;
const LOG_BODY_BUDGET: usize = 64 * 1024;
const SPOOL_RESERVE: u64 = 4096;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn corrupt(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[derive(Clone, Debug)]
pub struct Config {
    pub spool: PathBuf,
    pub logs: Vec<PathBuf>,
    pub interval_s: u64,
    pub spool_bytes: u64,
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let mut bytes = Vec::new();
        File::open(path)?
            .take(MAX_CONFIG_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err(invalid("node config exceeds 64 KiB"));
        }
        let text = String::from_utf8(bytes).map_err(|_| invalid("node config is not UTF-8"))?;
        let mut result = Config {
            spool: PathBuf::new(),
            logs: Vec::new(),
            interval_s: 15,
            spool_bytes: 256 * 1024 * 1024,
        };
        let mut seen_spool = false;
        let mut seen_interval = false;
        let mut seen_bytes = false;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| invalid("invalid node config line"))?;
            let value = value.trim();
            match key.trim() {
                "spool_dir" if !seen_spool => {
                    result.spool = PathBuf::from(value);
                    seen_spool = true;
                }
                "log" => result.logs.push(PathBuf::from(value)),
                "metric_interval_s" if !seen_interval => {
                    result.interval_s = value
                        .parse()
                        .map_err(|_| invalid("invalid metric interval"))?;
                    seen_interval = true;
                }
                "spool_bytes" if !seen_bytes => {
                    result.spool_bytes = value
                        .parse()
                        .map_err(|_| invalid("invalid spool byte ceiling"))?;
                    seen_bytes = true;
                }
                _ => return Err(invalid("unknown or duplicate node config key")),
            }
        }
        result.validate()?;
        result.logs.sort();
        result.logs.dedup();
        Ok(result)
    }

    fn validate(&self) -> io::Result<()> {
        if !self.spool.is_absolute()
            || self.spool.as_os_str().len() > 240
            || self.logs.len() > MAX_LOGS
            || self
                .logs
                .iter()
                .any(|p| !p.is_absolute() || p.as_os_str().len() > 240)
            || self.interval_s == 0
            || self.interval_s > 3600
            || self.spool_bytes < 8192
            || self.spool_bytes > 256 * 1024 * 1024
        {
            return Err(invalid("node config outside local profile"));
        }
        Ok(())
    }

    fn journal_cap(&self) -> u64 {
        self.spool_bytes - SPOOL_RESERVE
    }
}

fn now_ns() -> io::Result<u64> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| corrupt("system time before Unix epoch"))?
        .as_nanos();
    u64::try_from(nanos).map_err(|_| corrupt("Unix time overflow"))
}

fn attr(key: &str, value: impl Into<String>) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.into())),
        }),
        ..Default::default()
    }
}

fn resource(hostname: &str, boot_id: &str) -> Resource {
    Resource {
        attributes: vec![
            attr("host.name", hostname),
            attr("host.boot.id", boot_id),
            attr("service.name", "fabric-node"),
        ],
        ..Default::default()
    }
}

fn source_key(name: &str) -> &'static str {
    if name.starts_with("system.cpu.") {
        "state"
    } else if name.starts_with("system.disk.") {
        "device"
    } else if name.starts_with("system.network.") {
        "interface"
    } else if name.starts_with("system.filesystem.") {
        "mountpoint"
    } else {
        "host"
    }
}

fn history_key(boot_id: &str, name: &str, source: &str) -> String {
    format!("{boot_id}\0{name}\0{source}")
}

fn value_from_point(value: &Value) -> io::Result<number_data_point::Value> {
    match value {
        Value::Int(value) => Ok(number_data_point::Value::AsInt(
            i64::try_from(*value).map_err(|_| corrupt("counter exceeds OTLP signed integer"))?,
        )),
        Value::Double(value) if value.is_finite() => Ok(number_data_point::Value::AsDouble(*value)),
        _ => Err(corrupt("nonfinite host value")),
    }
}

fn decreased(current: &Value, previous: &Value) -> bool {
    match (current, previous) {
        (Value::Int(a), Value::Int(b)) => a < b,
        (Value::Double(a), Value::Double(b)) => a < b,
        _ => true,
    }
}

type History = BTreeMap<String, (Value, u64)>;

fn metric_request(
    snapshot: &host::Snapshot,
    now: u64,
    history: &History,
) -> io::Result<(Vec<u8>, History, usize)> {
    // A complete host sample replaces prior series. Retaining old device or
    // boot identities would make RAM grow across a bounded spool's lifetime.
    let mut next = History::new();
    let mut metrics = Vec::with_capacity(snapshot.points.len());
    for point in &snapshot.points {
        let key = history_key(&snapshot.boot_id, point.name, &point.source);
        let start = if point.kind == Kind::Counter {
            match history.get(&key) {
                Some((old, prior_start)) if !decreased(&point.value, old) => *prior_start,
                Some(_) => now,
                None => point.known_start_ns.unwrap_or(now),
            }
        } else {
            0
        };
        if point.kind == Kind::Counter {
            next.insert(key, (point.value.clone(), start));
        }
        let source = source_key(point.name);
        let attrs = if source == "host" {
            vec![]
        } else {
            vec![attr(source, &point.source)]
        };
        let datum = NumberDataPoint {
            attributes: attrs,
            start_time_unix_nano: start,
            time_unix_nano: now,
            value: Some(value_from_point(&point.value)?),
            ..Default::default()
        };
        let data = if point.kind == Kind::Counter {
            metric::Data::Sum(Sum {
                data_points: vec![datum],
                aggregation_temporality: AggregationTemporality::Cumulative as i32,
                is_monotonic: true,
            })
        } else {
            metric::Data::Gauge(Gauge {
                data_points: vec![datum],
            })
        };
        metrics.push(Metric {
            name: point.name.into(),
            unit: point.unit.into(),
            data: Some(data),
            ..Default::default()
        });
    }
    let count = metrics.len();
    let request = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(resource(&snapshot.hostname, &snapshot.boot_id)),
            scope_metrics: vec![ScopeMetrics {
                metrics,
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    Ok((request.encode_to_vec(), next, count))
}

fn encoded_logs(lines: &[log_source::Line], hostname: &str, boot_id: &str, now: u64) -> Vec<u8> {
    let records = lines
        .iter()
        .map(|line| LogRecord {
            observed_time_unix_nano: now,
            body: Some(AnyValue {
                value: Some(any_value::Value::StringValue(line.body.clone())),
            }),
            attributes: vec![
                attr("log.file.path", &line.path),
                attr("log.file.device", line.device.to_string()),
                attr("log.file.inode", line.inode.to_string()),
                attr("log.file.offset.start", line.start.to_string()),
                attr("log.file.offset.end", line.end.to_string()),
            ],
            ..Default::default()
        })
        .collect();
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(resource(hostname, boot_id)),
            scope_logs: vec![ScopeLogs {
                log_records: records,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec()
}

/// Truncate to the journal's per-gap byte cap on a UTF-8 character boundary.
fn bounded_gap(message: impl AsRef<str>) -> String {
    let message = message.as_ref();
    let mut end = message.len().min(MAX_GAP_BYTES);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message[..end].to_owned()
}

fn supported_counter(name: &str) -> bool {
    matches!(
        name,
        "system.cpu.time"
            | "system.disk.read.bytes"
            | "system.disk.write.bytes"
            | "system.network.receive.bytes"
            | "system.network.transmit.bytes"
    )
}

fn absorb(
    batch: Batch,
    allowed_logs: &BTreeSet<String>,
    cursors: &mut BTreeMap<String, Cursor>,
    history: &mut History,
) -> io::Result<()> {
    for cursor in batch.cursors {
        if allowed_logs.contains(&cursor.path) {
            cursors.insert(cursor.path.clone(), cursor);
        }
    }
    if batch.metrics.is_empty() {
        return Ok(());
    }
    let request = ExportMetricsServiceRequest::decode(batch.metrics.as_slice())
        .map_err(|_| corrupt("invalid committed metrics"))?;
    let mut current = History::new();
    for group in request.resource_metrics {
        let boot = group
            .resource
            .as_ref()
            .and_then(|r| r.attributes.iter().find(|a| a.key == "host.boot.id"))
            .and_then(|a| a.value.as_ref())
            .and_then(|v| v.value.as_ref())
            .and_then(|v| match v {
                any_value::Value::StringValue(s) => Some(s.as_str()),
                _ => None,
            })
            .filter(|boot| boot.len() == 36)
            .ok_or_else(|| corrupt("committed metrics missing boot ID"))?;
        for scope in group.scope_metrics {
            for metric in scope.metrics {
                if !supported_counter(&metric.name)
                    && matches!(&metric.data, Some(metric::Data::Sum(_)))
                {
                    return Err(corrupt("unsupported committed counter"));
                }
                let Some(metric::Data::Sum(sum)) = metric.data else {
                    continue;
                };
                for point in sum.data_points {
                    if current.len() >= 300 {
                        return Err(corrupt("committed counter cardinality cap"));
                    }
                    let source_name = source_key(&metric.name);
                    let source = point
                        .attributes
                        .iter()
                        .find(|a| a.key == source_name)
                        .and_then(|a| a.value.as_ref())
                        .and_then(|v| v.value.as_ref())
                        .and_then(|v| match v {
                            any_value::Value::StringValue(s) => Some(s.as_str()),
                            _ => None,
                        })
                        .unwrap_or("host");
                    if source.len() > 64 {
                        return Err(corrupt("committed counter source cap"));
                    }
                    let value = match point.value {
                        Some(number_data_point::Value::AsInt(v)) if v >= 0 => Value::Int(v as u64),
                        Some(number_data_point::Value::AsDouble(v)) if v.is_finite() => {
                            Value::Double(v)
                        }
                        _ => return Err(corrupt("invalid committed counter")),
                    };
                    current.insert(
                        history_key(boot, &metric.name, source),
                        (value, point.start_time_unix_nano),
                    );
                }
            }
        }
    }
    *history = current;
    Ok(())
}

pub struct Cycle {
    pub batch_sequence: u64,
    pub metric_points: usize,
    pub log_records: usize,
    pub gaps: usize,
    pub spool_bytes: u64,
}

pub struct Node {
    config: Config,
    journal: Journal,
    cursors: BTreeMap<String, Cursor>,
    history: History,
    host_paths: Paths,
}

impl Node {
    pub fn open(config: Config) -> io::Result<Self> {
        Self::open_with_paths(config, Paths::default())
    }

    pub fn open_with_paths(mut config: Config, host_paths: Paths) -> io::Result<Self> {
        config.validate()?;
        config.logs.sort();
        config.logs.dedup();
        let mut journal = Journal::open(&config.spool, config.journal_cap())?;
        let mut cursors = BTreeMap::new();
        let mut history = History::new();
        let allowed_logs: BTreeSet<String> = config
            .logs
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect();
        journal.replay(|batch| absorb(batch, &allowed_logs, &mut cursors, &mut history))?;
        Ok(Self {
            config,
            journal,
            cursors,
            history,
            host_paths,
        })
    }

    pub fn collect_once(&mut self) -> io::Result<Cycle> {
        let now = now_ns()?;
        let mut gaps = Vec::new();
        // A prior cycle could not commit. Its interval is reported as a gap in
        // the next committed batch rather than halting collection for good.
        let unknown_since = read_unknown(&self.config.spool)?;
        if let Some(since) = unknown_since {
            gaps.push(format!(
                "coverage unknown since {since} ns: an earlier collection did not commit"
            ));
        }
        let sampled = match host::sample(&self.host_paths) {
            Ok(snapshot) => Some(snapshot),
            Err(error) => {
                gaps.push(bounded_gap(format!("host metrics unavailable: {error}")));
                None
            }
        };
        let (metrics, updated_history, metric_points) = if let Some(snapshot) = sampled.as_ref() {
            let (bytes, history, count) = metric_request(snapshot, now, &self.history)?;
            (bytes, history, count)
        } else {
            (Vec::new(), self.history.clone(), 0)
        };
        let mut lines = Vec::new();
        let mut pending_cursors = Vec::new();
        let mut remaining = LOG_BODY_BUDGET;
        for path in &self.config.logs {
            let name = path.to_string_lossy().to_string();
            match log_source::read_lines(path, self.cursors.get(&name), remaining) {
                Ok(read) => {
                    remaining = remaining.saturating_sub(
                        read.lines.iter().map(|line| line.body.len()).sum::<usize>(),
                    );
                    lines.extend(read.lines);
                    pending_cursors.push(read.cursor);
                    gaps.extend(read.gaps.into_iter().map(bounded_gap));
                }
                Err(error) => gaps.push(bounded_gap(format!(
                    "log source unavailable {name}: {error}"
                ))),
            }
        }
        debug_assert!(gaps.len() <= MAX_GAPS_PER_BATCH);
        let (hostname, boot_id) = sampled
            .as_ref()
            .map(|s| (s.hostname.as_str(), s.boot_id.as_str()))
            .unwrap_or(("unknown", "unknown"));
        let logs = if lines.is_empty() {
            Vec::new()
        } else {
            encoded_logs(&lines, hostname, boot_id, now)
        };
        let candidate = Batch {
            version: 1,
            node_id: vec![],
            generation: 0,
            sequence: 0,
            metrics,
            logs,
            cursors: pending_cursors,
            collection_gaps: gaps,
        };
        let committed = match self.journal.append(&candidate) {
            Ok(batch) => batch,
            Err(error) => {
                // The candidate and source cursor remain ours on failure.
                // Coverage cannot be called delivered; persist unknown state.
                if let Err(marker) = mark_unknown(&self.config.spool, now) {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("{error}; writing coverage-unknown also failed: {marker}"),
                    ));
                }
                return Err(error);
            }
        };
        if unknown_since.is_some() {
            clear_unknown(&self.config.spool)?;
        }
        let sequence = committed.sequence;
        let gap_count = committed.collection_gaps.len();
        for cursor in committed.cursors {
            self.cursors.insert(cursor.path.clone(), cursor);
        }
        self.history = updated_history;
        Ok(Cycle {
            batch_sequence: sequence,
            metric_points,
            log_records: lines.len(),
            gaps: gap_count,
            spool_bytes: self.journal.used_bytes(),
        })
    }
}

const UNKNOWN_MARKER: &str = "coverage-unknown";

/// Keep the earliest failed attempt; later failures extend the same interval.
fn mark_unknown(dir: &Path, since_ns: u64) -> io::Result<()> {
    let path = dir.join(UNKNOWN_MARKER);
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            writeln!(file, "{since_ns}")?;
            file.sync_all()?;
            File::open(dir)?.sync_all()?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    Ok(())
}

fn read_unknown(dir: &Path) -> io::Result<Option<u64>> {
    let mut text = String::new();
    match File::open(dir.join(UNKNOWN_MARKER)) {
        Ok(file) => file.take(32).read_to_string(&mut text)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    text.trim()
        .parse()
        .map(Some)
        .map_err(|_| corrupt("invalid coverage-unknown marker"))
}

/// Called only after the batch carrying the gap notice has committed.
fn clear_unknown(dir: &Path) -> io::Result<()> {
    std::fs::remove_file(dir.join(UNKNOWN_MARKER))?;
    File::open(dir)?.sync_all()
}

pub struct Report {
    pub node_id: [u8; 16],
    pub generation: u64,
    pub batches: usize,
    pub metric_points: usize,
    pub log_records: usize,
    pub otlp_payload_bytes: u64,
    pub gaps: usize,
    pub committed_bytes: u64,
    pub file_bytes: u64,
    pub coverage_unknown: bool,
    pub recovery_required: bool,
    pub interrupted_append: bool,
}

pub fn inspect(config: &Config) -> io::Result<Report> {
    config.validate()?;
    let mut batches = 0;
    let mut metric_points = 0;
    let mut log_records = 0;
    let mut otlp_payload_bytes = 0_u64;
    let mut gaps = 0;
    let info = Journal::inspect(&config.spool, config.journal_cap(), |batch| {
        batches += 1;
        gaps += batch.collection_gaps.len();
        otlp_payload_bytes = otlp_payload_bytes
            .checked_add((batch.metrics.len() + batch.logs.len()) as u64)
            .ok_or_else(|| corrupt("OTLP payload byte count overflow"))?;
        if !batch.metrics.is_empty() {
            let request = ExportMetricsServiceRequest::decode(batch.metrics.as_slice())
                .map_err(|_| corrupt("invalid committed OTLP metrics"))?;
            for group in request.resource_metrics {
                for scope in group.scope_metrics {
                    for metric in scope.metrics {
                        metric_points += match metric.data {
                            Some(metric::Data::Gauge(g)) => g.data_points.len(),
                            Some(metric::Data::Sum(s)) => s.data_points.len(),
                            _ => return Err(corrupt("unsupported committed metric kind")),
                        };
                    }
                }
            }
        }
        if !batch.logs.is_empty() {
            let request = ExportLogsServiceRequest::decode(batch.logs.as_slice())
                .map_err(|_| corrupt("invalid committed OTLP logs"))?;
            for group in request.resource_logs {
                for scope in group.scope_logs {
                    log_records += scope.log_records.len();
                }
            }
        }
        Ok(())
    })?;
    let coverage_unknown = config.spool.join(UNKNOWN_MARKER).exists();
    Ok(Report {
        node_id: info.node_id,
        generation: info.generation,
        batches,
        metric_points,
        log_records,
        otlp_payload_bytes,
        gaps,
        committed_bytes: info.committed_bytes,
        file_bytes: info.file_bytes,
        coverage_unknown,
        recovery_required: info.recovery_required,
        interrupted_append: info.interrupted_append,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_metric_series_replace_obsolete_boot_and_device_keys() {
        let mut history = History::new();
        let mut recovered = History::new();
        let mut cursors = BTreeMap::new();
        let allowed = BTreeSet::from(["/configured.log".to_owned()]);
        for old in 0..1000 {
            history.insert(format!("old-boot-{old}"), (Value::Int(99), 1));
        }
        for boot in 0..32 {
            let snapshot = host::Snapshot {
                hostname: "fixture".into(),
                boot_id: format!("{boot:08x}-0000-0000-0000-000000000000"),
                points: vec![host::Point {
                    name: "system.disk.read.bytes",
                    unit: "By",
                    kind: Kind::Counter,
                    source: format!("disk-{boot}"),
                    value: Value::Int(boot),
                    known_start_ns: None,
                }],
            };
            let (metrics, next, _) = metric_request(&snapshot, 100 + boot, &history).unwrap();
            assert_eq!(next.len(), 1);
            let batch = Batch {
                version: 1,
                node_id: vec![1; 16],
                generation: 1,
                sequence: boot + 1,
                metrics,
                logs: vec![],
                collection_gaps: vec![],
                cursors: vec![Cursor {
                    path: format!("/removed-{boot}.log"),
                    device: 1,
                    inode: boot,
                    offset: 1,
                    skipping_oversize: false,
                    prefix_len: 0,
                    prefix_crc: 0,
                }],
            };
            absorb(batch, &allowed, &mut cursors, &mut recovered).unwrap();
            assert_eq!(recovered.len(), 1);
            assert!(cursors.is_empty());
            history = next;
        }
    }
}
