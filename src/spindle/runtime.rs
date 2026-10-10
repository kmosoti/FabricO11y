//! One-process native collector. Each cycle owns a candidate until journal commit.

use crate::spindle::host::{self, Kind, Paths, Value};
use crate::spindle::log_source;
use crate::spindle::sender::{Delivery, RemoteView, Sender, ServerTarget};
use crate::spindle::spool::{Batch, Cursor, MAX_GAP_BYTES, MAX_GAPS_PER_BATCH, Spool};
use fabric_core::collection::{CounterValue, bounded_text, counter_start};
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
use sha2::Digest;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const MAX_LOGS: usize = 16;
/// Log bytes per Batch, counting each line's encoding overhead (ADR-0025).
const LOG_BODY_BUDGET: usize = fabric_frame::envelope::MAX_BATCH - BATCH_RESERVE;
/// Encoding overhead of one log record beyond its body and path: the observed
/// time, four numeric attributes with their keys, and the protobuf framing. The
/// encoded record is at most body + path + this many bytes (see the test
/// `a_full_batch_of_short_lines_stays_under_the_envelope_cap`).
const LOG_LINE_OVERHEAD: usize = 256;
/// Room kept in every Batch for the resource, the cursors and the gaps.
const BATCH_RESERVE: usize = 96 * 1024;
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
    /// Delivery target; `None` keeps every batch local.
    pub server: Option<ServerTarget>,
    /// Loopback address of the OTLP/HTTP trace endpoint (ADR-0025); `None` disables it.
    pub traces_listen: Option<std::net::SocketAddr>,
    /// Cap on delivered Batch bytes per second (`max_output_bytes_per_s`); `None` is
    /// uncapped. At least 64 KiB/s when set.
    pub max_output_bytes_per_s: Option<u64>,
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
            server: None,
            traces_listen: None,
            max_output_bytes_per_s: None,
        };
        let (mut url, mut ca, mut token_file) = (None, None, None);
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
                "server_url" if url.is_none() => url = Some(value.to_owned()),
                "server_ca" if ca.is_none() => ca = Some(PathBuf::from(value)),
                "token_file" if token_file.is_none() => token_file = Some(PathBuf::from(value)),
                "max_output_bytes_per_s" if result.max_output_bytes_per_s.is_none() => {
                    let rate: u64 = value
                        .parse()
                        .map_err(|_| invalid("invalid max_output_bytes_per_s"))?;
                    if rate < 64 * 1024 {
                        return Err(invalid("max_output_bytes_per_s must be at least 65536"));
                    }
                    result.max_output_bytes_per_s = Some(rate);
                }
                "traces_listen" if result.traces_listen.is_none() => {
                    result.traces_listen = Some(super::otlp::check_listen(value)?);
                }
                _ => return Err(invalid("unknown or duplicate node config key")),
            }
        }
        result.server = match (url, ca, token_file) {
            (None, None, None) => None,
            (Some(url), Some(ca), Some(token_file)) => Some(ServerTarget {
                url,
                ca,
                token_file,
            }),
            _ => {
                return Err(invalid(
                    "server_url, server_ca and token_file must be set together",
                ));
            }
        };
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
        if let Some(server) = &self.server {
            server.validate()?;
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

fn counter_value(value: &Value) -> CounterValue {
    match value {
        Value::Int(v) => CounterValue::Int(*v),
        Value::Double(v) => CounterValue::Double(*v),
    }
}

type History = BTreeMap<String, (Value, u64)>;

fn metric_request(
    snapshot: &host::Snapshot,
    now: u64,
    history: &History,
    extra: Vec<Metric>,
) -> io::Result<(Vec<u8>, History, usize)> {
    #[cfg(feature = "phase-probe")]
    let _phase = fabric_frame::probe::span("native_otlp_metrics_assembly");

    // A complete host sample replaces prior series. Retaining old device or
    // boot identities would make RAM grow across a bounded spool's lifetime.
    let mut next = History::new();
    let mut metrics = Vec::with_capacity(snapshot.points.len());
    for point in &snapshot.points {
        let key = history_key(&snapshot.boot_id, point.name, &point.source);
        let start = if point.kind == Kind::Counter {
            counter_start(
                history
                    .get(&key)
                    .map(|(old, prior_start)| (counter_value(old), *prior_start)),
                counter_value(&point.value),
                point.known_start_ns,
                now,
            )
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
    // Host points only: the meter's own metrics ride along uncounted.
    let count = metrics.len();
    metrics.extend(extra);
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

fn owned_log_record(line: log_source::Line, now: u64) -> LogRecord {
    LogRecord {
        observed_time_unix_nano: now,
        body: Some(AnyValue {
            value: Some(any_value::Value::StringValue(line.body)),
        }),
        attributes: vec![
            attr("log.file.path", line.path),
            attr("log.file.device", line.device.to_string()),
            attr("log.file.inode", line.inode.to_string()),
            attr("log.file.offset.start", line.start.to_string()),
            attr("log.file.offset.end", line.end.to_string()),
        ],
        ..Default::default()
    }
}

fn encoded_logs(lines: Vec<log_source::Line>, hostname: &str, boot_id: &str, now: u64) -> Vec<u8> {
    #[cfg(feature = "phase-probe")]
    let _phase = fabric_frame::probe::span("native_otlp_logs_assembly");

    let records = lines
        .into_iter()
        .map(|line| owned_log_record(line, now))
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
    bounded_text(message.as_ref(), MAX_GAP_BYTES).to_owned()
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
                // The Spindle's own output meter restarts from zero with the
                // process; there is no counter history to restore for it.
                if metric.name.starts_with(super::meter::PREFIX) {
                    continue;
                }
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
    /// Unread bytes across configured logs after this cycle's reads.
    pub log_backlog_bytes: u64,
}

/// Optional operational timing, independent of telemetry and durable state.
/// A wall sample lies within the monotonic bracket; a failed wall read is absent.
#[derive(Clone, Copy, Debug)]
pub struct TimingStamp {
    pub unix_ns: Option<u64>,
    /// Linux CLOCK_MONOTONIC, shared by producer processes within this boot.
    /// Cross-host visibility still needs a measured correlation bracket.
    pub boot_monotonic_ns: Option<u64>,
    pub monotonic_before_ns: u64,
    pub monotonic_after_ns: u64,
}

fn timing_stamp(epoch: std::time::Instant) -> TimingStamp {
    timing_stamp_read(epoch, now_ns)
}

fn timing_stamp_read(
    epoch: std::time::Instant,
    read: impl FnOnce() -> io::Result<u64>,
) -> TimingStamp {
    let monotonic_before_ns = epoch.elapsed().as_nanos().min(u64::MAX as u128) as u64;
    let unix_ns = read().ok();
    let boot_monotonic_ns = boot_monotonic_ns();
    let monotonic_after_ns = epoch.elapsed().as_nanos().min(u64::MAX as u128) as u64;
    TimingStamp {
        unix_ns,
        boot_monotonic_ns,
        monotonic_before_ns,
        monotonic_after_ns,
    }
}

fn boot_monotonic_ns() -> Option<u64> {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: value is writable timespec storage and CLOCK_MONOTONIC is a
    // Linux clock ID. Failure stays absent operational evidence.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut value) } != 0 {
        return None;
    }
    checked_clock_ns(value.tv_sec, value.tv_nsec)
}

fn checked_clock_ns(seconds: libc::time_t, nanos: libc::c_long) -> Option<u64> {
    let seconds = u64::try_from(seconds).ok()?;
    let nanos = u64::try_from(nanos).ok()?;
    if nanos >= 1_000_000_000 {
        return None;
    }
    seconds.checked_mul(1_000_000_000)?.checked_add(nanos)
}

#[derive(Debug)]
pub struct TimingEvent {
    pub node_id: [u8; 16],
    pub generation: u64,
    pub sequence: u64,
    pub stage: &'static str,
    pub stamp: TimingStamp,
}

/// The Spindle runtime: one host's collection state, Spool and delivery client.
pub struct Spindle {
    /// The configuration in force: the local base with any applied remote view.
    config: Config,
    base: Config,
    /// Local composition-root sources cannot be removed by remote configuration.
    local_logs: Vec<PathBuf>,
    applied: Option<RemoteView>,
    config_error: Option<String>,
    journal: Spool,
    cursors: BTreeMap<String, Cursor>,
    history: History,
    host_paths: Paths,
    sender: Option<Sender>,
    /// A coverage-unknown notice already committed while its marker remains.
    unknown_reported: Option<Unknown>,
    /// Output metering and the optional delivery rate cap.
    meter: super::meter::Meter,
    /// Unread log bytes after the last pass, for the meter's gauge.
    last_backlog: u64,
    timing_epoch: std::time::Instant,
    timing_enabled: bool,
    timing_events: VecDeque<TimingEvent>,
    timing_dropped: u64,
}

/// What one configuration poll did.
#[derive(Debug, Default)]
pub struct ConfigPoll {
    /// A new configuration was validated, stored and activated.
    pub changed: bool,
    /// Why the poll or the new configuration was not used.
    pub error: Option<String>,
}

const APPLIED: &str = "applied-config.json";

/// Merge local sources without increasing the collection profile's bound.
fn with_local_logs(mut config: Config, local_logs: &[PathBuf]) -> io::Result<Config> {
    config.validate()?;
    config.logs.extend_from_slice(local_logs);
    config.logs.sort();
    config.logs.dedup();
    config.validate()?;
    Ok(config)
}

/// The local base with a remote view's log paths and interval, retaining pins.
fn effective(base: &Config, view: &RemoteView, local_logs: &[PathBuf]) -> io::Result<Config> {
    let mut config = base.clone();
    config.logs = view.logs.iter().map(PathBuf::from).collect();
    config.interval_s = view.metric_interval_s;
    with_local_logs(config, local_logs)
}

fn read_applied(dir: &Path) -> Option<RemoteView> {
    let file = File::open(dir.join(APPLIED)).ok()?;
    let mut text = String::new();
    file.take(MAX_CONFIG_BYTES).read_to_string(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

/// Stored by synced rename before activation, so a restart runs it too.
fn write_applied(dir: &Path, view: &RemoteView) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let staged = dir.join("applied-config.json.tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&staged)?;
    file.write_all(&serde_json::to_vec(view).map_err(|e| io::Error::other(e.to_string()))?)?;
    file.sync_all()?;
    std::fs::rename(&staged, dir.join(APPLIED))?;
    File::open(dir)?.sync_all()
}

/// One send attempt: the sequence, SHA-256 of the exact bytes sent, and the answer.
#[derive(Debug)]
pub struct Attempt {
    pub sequence: u64,
    pub sha256: String,
    pub outcome: Delivery,
    /// Attempt start to answer, including any rate-cap wait; for an `ack`
    /// this includes the server's durable commit. It is not creation-to-ACK.
    pub elapsed_us: u64,
}

/// What one delivery call achieved.
#[derive(Debug)]
pub struct DeliveryReport {
    pub sent: usize,
    pub acked_through: u64,
    /// No unacknowledged batch remains.
    pub caught_up: bool,
    /// Why delivery stopped early, if it did.
    pub error: Option<String>,
}

/// One experimental send and at most one durably prepared successor.
pub struct OverlapReport {
    pub delivery: DeliveryReport,
    pub prepared: Option<Cycle>,
    /// Wall-clock sample immediately after successful durable collection, before
    /// joining the request worker; not the marker syscall's exact finish time.
    pub prepared_unix_ns: Option<u64>,
    pub collection_error: Option<String>,
}

impl Spindle {
    /// Opt-in evidence only. The fixed buffer never delays or fails a commit.
    pub fn enable_timing_events(&mut self) {
        self.timing_enabled = true;
    }

    pub fn take_timing_events(&mut self) -> (Vec<TimingEvent>, u64) {
        let dropped = std::mem::take(&mut self.timing_dropped);
        (self.timing_events.drain(..).collect(), dropped)
    }

    fn timing_sample(&self) -> Option<TimingStamp> {
        self.timing_enabled.then(|| timing_stamp(self.timing_epoch))
    }

    fn timing(&mut self, sequence: u64, stage: &'static str, stamp: Option<TimingStamp>) {
        let Some(stamp) = stamp else {
            return;
        };
        if self.timing_events.len() == 256 {
            self.timing_dropped = self.timing_dropped.saturating_add(1);
            return;
        }
        let (node_id, generation) = self.journal.identity();
        self.timing_events.push_back(TimingEvent {
            node_id,
            generation,
            sequence,
            stage,
            stamp,
        });
    }
    /// Experimental logs/metrics overlap. The caller owns scheduling/config polls;
    /// this call sends only the oldest unacknowledged Batch and joins its worker
    /// before processing ACK or returning. It never sends the prepared successor.
    #[doc(hidden)]
    pub fn deliver_with_one_prepared(
        &mut self,
        until: std::time::Instant,
        include_metrics: bool,
        stop: &std::sync::atomic::AtomicBool,
        on_attempt: impl FnMut(&Attempt),
    ) -> io::Result<OverlapReport> {
        if self.config.traces_listen.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "overlap is scoped to logs/metrics without a trace listener",
            ));
        }
        let Some(sender) = self.sender.take() else {
            return Ok(OverlapReport {
                delivery: DeliveryReport {
                    sent: 0,
                    acked_through: self.journal.acked_through(),
                    caught_up: true,
                    error: None,
                },
                prepared: None,
                prepared_unix_ns: None,
                collection_error: None,
            });
        };
        let result = self.overlap_attempt(until, include_metrics, stop, on_attempt, |bytes| {
            sender.send(bytes)
        });
        self.sender = Some(sender);
        result
    }

    fn overlap_attempt(
        &mut self,
        until: std::time::Instant,
        include_metrics: bool,
        stop: &std::sync::atomic::AtomicBool,
        mut on_attempt: impl FnMut(&Attempt),
        send: impl FnOnce(&[u8]) -> Delivery + Send,
    ) -> io::Result<OverlapReport> {
        use std::sync::atomic::Ordering;
        let mut report = OverlapReport {
            delivery: DeliveryReport {
                sent: 0,
                acked_through: self.journal.acked_through(),
                caught_up: false,
                error: None,
            },
            prepared: None,
            prepared_unix_ns: None,
            collection_error: None,
        };
        if stop.load(Ordering::SeqCst) || std::time::Instant::now() >= until {
            return Ok(report);
        }
        let Some((sequence, bytes)) = self.journal.next_unacked()? else {
            report.delivery.caught_up = true;
            return Ok(report);
        };
        let length = bytes.len();
        let sha256: String = sha2::Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let started = std::time::Instant::now();
        let wait = self.meter.take(length);
        if !wait.is_zero() {
            let now = std::time::Instant::now();
            if now + wait > until {
                self.meter.untake(length);
                std::thread::sleep(until.saturating_duration_since(now));
                return Ok(report);
            }
            std::thread::sleep(wait);
        }
        if stop.load(Ordering::SeqCst) {
            self.meter.untake(length);
            return Ok(report);
        }
        // A retry/backlog with an already durable N+1 must never prepare N+2.
        let prior_last = self.journal.next_sequence() - 1;
        let prepare = prior_last == sequence;
        let (outcome, elapsed_us) = std::thread::scope(|scope| {
            let worker = scope.spawn(move || {
                let outcome = send(&bytes);
                (outcome, started.elapsed().as_micros() as u64)
            });
            if prepare && !stop.load(Ordering::SeqCst) {
                match self.collect(include_metrics) {
                    Ok(cycle) => {
                        if cycle.is_some() {
                            report.prepared_unix_ns = now_ns().ok();
                        }
                        report.prepared = cycle;
                    }
                    Err(error) => report.collection_error = Some(error.to_string()),
                }
            }
            worker
                .join()
                .map_err(|_| io::Error::other("overlap send worker panicked"))
        })?;
        report.delivery.sent = 1;
        on_attempt(&Attempt {
            sequence,
            sha256,
            outcome: outcome.clone(),
            elapsed_us,
        });
        match outcome {
            Delivery::Ack(through) if through > prior_last => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "server acknowledged beyond the pre-attempt durable sequence",
                ));
            }
            Delivery::Ack(through) if through >= sequence => {
                self.journal.record_ack(through)?;
                self.meter.record_delivery(length);
                report.delivery.acked_through = through;
            }
            Delivery::Ack(through) => {
                report.delivery.error = Some(format!(
                    "server acknowledged {through}, below sent sequence {sequence}"
                ))
            }
            Delivery::Conflict(through) => {
                report.delivery.error = Some(format!(
                    "server holds different bytes for sequence {sequence} (committed through {through}); delivery stopped"
                ))
            }
            Delivery::Gap(through) => {
                report.delivery.error = Some(format!(
                    "server committed only through {through}, below this node's acknowledged {}; delivery stopped",
                    self.journal.acked_through()
                ))
            }
            Delivery::Rejected(why) | Delivery::Retry(why) => report.delivery.error = Some(why),
        }
        report.delivery.caught_up = self.journal.next_unacked()?.is_none();
        Ok(report)
    }

    pub fn open(config: Config) -> io::Result<Self> {
        Self::open_with_paths(config, Paths::default())
    }

    /// Pin local diagnostic sources across remote log-path replacement. All
    /// sources together share the existing sixteen-path collection bound.
    pub fn open_with_local_logs(config: Config, local_logs: Vec<PathBuf>) -> io::Result<Self> {
        Self::open_inner(config, Paths::default(), local_logs)
    }

    pub fn open_with_paths(config: Config, host_paths: Paths) -> io::Result<Self> {
        Self::open_inner(config, host_paths, Vec::new())
    }

    fn open_inner(
        mut config: Config,
        host_paths: Paths,
        mut local_logs: Vec<PathBuf>,
    ) -> io::Result<Self> {
        config.validate()?;
        config.logs.sort();
        config.logs.dedup();
        local_logs.sort();
        local_logs.dedup();
        // The local file is the base: spool, ceiling and server target. The
        // last configuration applied from the server, if still valid against
        // that base, replaces its log paths and interval (ADR-0014).
        let base = config.clone();
        config = with_local_logs(config, &local_logs)?;
        let mut applied = None;
        let mut config_error = None;
        if let Some(view) = read_applied(&config.spool) {
            match effective(&base, &view, &local_logs) {
                Ok(effective) => {
                    config = effective;
                    applied = Some(view);
                }
                Err(error) => {
                    config_error = Some(format!("stored revision {}: {error}", view.revision));
                }
            }
        }
        let mut journal = Spool::open(&config.spool, config.journal_cap())?;
        let mut cursors = BTreeMap::new();
        let mut history = History::new();
        let allowed_logs: BTreeSet<String> = config
            .logs
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect();
        let mut newest_gaps = Vec::new();
        journal.replay(|batch| {
            newest_gaps.clone_from(&batch.collection_gaps);
            absorb(batch, &allowed_logs, &mut cursors, &mut history)
        })?;
        let sender = config.server.as_ref().map(Sender::new).transpose()?;
        // A kill between committing a notice and removing its marker leaves
        // the marker behind. A timestamped notice is unique, so if the newest
        // batch already carries it, it has been reported.
        let mut unknown_reported = None;
        if let Some(unknown @ Unknown::Since(_)) = read_unknown(&config.spool)
            && newest_gaps.contains(&unknown.notice())
        {
            unknown_reported = Some(unknown);
            if clear_unknown(&config.spool).is_ok() {
                unknown_reported = None;
            }
        }
        let config_rate = config.max_output_bytes_per_s;
        Ok(Self {
            config,
            base,
            local_logs,
            applied,
            config_error,
            journal,
            cursors,
            history,
            host_paths,
            sender,
            unknown_reported,
            meter: super::meter::Meter::new(
                now_ns()?,
                config_rate,
                fabric_frame::envelope::MAX_BATCH,
            ),
            last_backlog: 0,
            timing_epoch: std::time::Instant::now(),
            timing_enabled: false,
            timing_events: VecDeque::new(),
            timing_dropped: 0,
        })
    }

    pub fn acked_through(&self) -> u64 {
        self.journal.acked_through()
    }

    pub fn paused(&self) -> bool {
        self.applied.as_ref().is_some_and(|v| v.paused)
    }

    pub fn interval_s(&self) -> u64 {
        self.config.interval_s
    }

    pub fn applied_revision(&self) -> u64 {
        self.applied.as_ref().map_or(0, |v| v.revision)
    }

    /// Ask the server for configuration and activate a new valid view.
    /// Transport and validation problems are reported, never fatal: the node
    /// keeps running its last applied configuration.
    pub fn poll_config(&mut self) -> io::Result<ConfigPoll> {
        let Some(sender) = self.sender.as_ref() else {
            return Ok(ConfigPoll::default());
        };
        let view = match sender.fetch_config(self.applied_revision(), self.config_error.as_deref())
        {
            Ok(Some(view)) => view,
            Ok(None) => return Ok(ConfigPoll::default()),
            Err(error) => {
                return Ok(ConfigPoll {
                    changed: false,
                    error: Some(error),
                });
            }
        };
        let effective = match effective(&self.base, &view, &self.local_logs) {
            Ok(effective) => effective,
            Err(error) => {
                let message = format!("revision {}: {error}", view.revision);
                self.config_error = Some(message.clone());
                return Ok(ConfigPoll {
                    changed: false,
                    error: Some(message),
                });
            }
        };
        write_applied(&self.config.spool, &view)?;
        if view.paused && !self.paused() {
            // The paused interval is a collection gap; the next batch after
            // resume reports it through the coverage-unknown path.
            mark_unknown(
                &self.config.spool,
                now_ns()?,
                self.unknown_reported.is_some(),
            )?;
            self.unknown_reported = None;
        }
        self.config = effective;
        self.applied = Some(view);
        self.config_error = None;
        Ok(ConfigPoll {
            changed: true,
            error: None,
        })
    }

    /// Send unacknowledged batches in order until caught up, `until` passes,
    /// or the server does not acknowledge. Without a server this is a no-op.
    /// `on_attempt` sees each answer before any ACK cursor is persisted, so an
    /// observer's record of acknowledgements never trails the spool's cursor.
    pub fn deliver(
        &mut self,
        until: std::time::Instant,
        mut on_attempt: impl FnMut(&Attempt),
    ) -> io::Result<DeliveryReport> {
        let mut report = DeliveryReport {
            sent: 0,
            acked_through: self.journal.acked_through(),
            caught_up: false,
            error: None,
        };
        if self.sender.is_none() {
            report.caught_up = true;
            return Ok(report);
        }
        while std::time::Instant::now() < until {
            let Some((sequence, bytes)) = self.journal.next_unacked()? else {
                report.caught_up = true;
                break;
            };
            report.sent += 1;
            let started = std::time::Instant::now();
            // The rate cap delays a send; a wait past `until` ends this delivery
            // slice instead (the tokens are given back), so the loop never spins.
            let wait = self.meter.take(bytes.len());
            if !wait.is_zero() {
                let now = std::time::Instant::now();
                if now + wait > until {
                    self.meter.untake(bytes.len());
                    std::thread::sleep(until.saturating_duration_since(now));
                    report.sent -= 1;
                    break;
                }
                std::thread::sleep(wait);
            }
            let send_stamp = self.timing_sample();
            let outcome = self
                .sender
                .as_ref()
                .expect("sender checked above")
                .send(&bytes);
            let answer_stamp = self.timing_sample();
            let elapsed_us = started.elapsed().as_micros() as u64;
            on_attempt(&Attempt {
                elapsed_us,
                sequence,
                sha256: sha2::Sha256::digest(&bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
                outcome: outcome.clone(),
            });
            self.timing(sequence, "send_started", send_stamp);
            self.timing(sequence, "answer_received", answer_stamp);
            match outcome {
                Delivery::Ack(through) if through >= sequence => {
                    // Beyond our last committed sequence is an error from
                    // record_ack; the spool is left unchanged.
                    self.journal.record_ack(through)?;
                    self.meter.record_delivery(bytes.len());
                    report.acked_through = through;
                }
                Delivery::Ack(through) => {
                    report.error = Some(format!(
                        "server acknowledged {through}, below sent sequence {sequence}"
                    ));
                    break;
                }
                Delivery::Conflict(through) => {
                    report.error = Some(format!(
                        "server holds different bytes for sequence {sequence} (committed through {through}); delivery stopped"
                    ));
                    break;
                }
                Delivery::Gap(through) => {
                    report.error = Some(format!(
                        "server committed only through {through}, below this node's acknowledged {}; delivery stopped",
                        self.journal.acked_through()
                    ));
                    break;
                }
                Delivery::Rejected(why) | Delivery::Retry(why) => {
                    report.error = Some(why);
                    break;
                }
            }
        }
        Ok(report)
    }

    /// Commit trace exports (encoded `ExportTraceServiceRequest`s, already validated)
    /// as one Batch. Concatenated encodings of a message are its merge, so the Batch
    /// holds every span of every export. The Batch carries the committed log cursors
    /// forward, as every Batch does, because the Spool restores cursors from its
    /// newest Batches. Returns the Batch sequence.
    pub fn commit_traces(&mut self, exports: &[&[u8]]) -> io::Result<u64> {
        let traces: Vec<u8> = exports.concat();
        let candidate = Batch {
            version: 1,
            node_id: vec![],
            generation: 0,
            sequence: 0,
            metrics: Vec::new(),
            logs: Vec::new(),
            cursors: self.cursors.values().cloned().collect(),
            collection_gaps: Vec::new(),
            traces,
        };
        let spans: usize = exports
            .iter()
            .filter_map(|e| {
                opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest::decode(
                    *e,
                )
                .ok()
            })
            .flat_map(|r| r.resource_spans)
            .flat_map(|r| r.scope_spans)
            .map(|s| s.spans.len())
            .sum();
        let accepted_stamp = self.timing_sample();
        let committed = self.journal.append_owned(candidate)?;
        let committed_stamp = self.timing_sample();
        self.timing(committed.sequence, "traces_accepted", accepted_stamp);
        self.timing(committed.sequence, "spool_committed", committed_stamp);
        self.meter.record_commit(super::meter::Committed {
            traces: (committed.traces.len() as u64, spans as u64),
            ..Default::default()
        });
        Ok(committed.sequence)
    }

    /// One full cycle: host metrics and every configured log. Always commits
    /// a batch (metrics, lines or at least a gap).
    pub fn collect_once(&mut self) -> io::Result<Cycle> {
        #[cfg(feature = "phase-probe")]
        let _phase = fabric_frame::probe::span("native_collection_inclusive");

        if self.paused() {
            return Err(io::Error::other("collection is paused by the server"));
        }
        self.collect(true)?
            .ok_or_else(|| io::Error::other("metrics cycle produced no batch"))
    }

    /// Read configured logs. New lines, gaps or forward progress through an
    /// oversized line or stable-identity migration commit a Batch. Cursor-only
    /// progress may sample metrics;
    /// a poll with no progress writes nothing to the Spool.
    pub fn collect_logs(&mut self) -> io::Result<Option<Cycle>> {
        self.collect(false)
    }

    fn sample_collection_host(&self, gaps: &mut Vec<String>) -> Option<host::Snapshot> {
        let observation = {
            #[cfg(feature = "phase-probe")]
            let _phase = fabric_frame::probe::span("native_host_observation");
            host::sample(&self.host_paths)
        };
        match observation {
            Ok(snapshot) => Some(snapshot),
            Err(error) => {
                gaps.push(bounded_gap(format!("host metrics unavailable: {error}")));
                None
            }
        }
    }

    fn collection_metrics(
        &self,
        sampled: Option<&host::Snapshot>,
        now: u64,
    ) -> io::Result<(Vec<u8>, Option<History>, usize)> {
        let Some(snapshot) = sampled else {
            return Ok((Vec::new(), None, 0));
        };
        let unacked = self
            .journal
            .next_sequence()
            .saturating_sub(1)
            .saturating_sub(self.journal.acked_through());
        let metered =
            self.meter
                .metrics(now, self.journal.used_bytes(), unacked, self.last_backlog);
        let (bytes, history, count) = metric_request(snapshot, now, &self.history, metered)?;
        Ok((bytes, Some(history), count))
    }

    fn collect(&mut self, include_metrics: bool) -> io::Result<Option<Cycle>> {
        self.collect_with_log_reader(include_metrics, log_source::read_lines_costed)
    }

    fn collect_with_log_reader(
        &mut self,
        include_metrics: bool,
        mut read_log: impl FnMut(
            &Path,
            Option<&Cursor>,
            usize,
            usize,
        ) -> io::Result<log_source::ReadResult>,
    ) -> io::Result<Option<Cycle>> {
        if self.paused() {
            return Ok(None);
        }
        // The Spool rotates only before a Batch with metrics, so every retained file
        // starts with counter state for replay. A log-heavy node would otherwise grow
        // one file until its next metric interval and could not reclaim acknowledged
        // bytes; sampling host metrics when rotation is due keeps files near 8 MiB.
        let include_metrics = include_metrics || self.journal.rotation_due();
        let now = now_ns()?;
        let observed_stamp = self.timing_sample();
        let mut gaps = Vec::new();
        // A prior cycle could not commit. Its interval is reported as a gap in
        // the next committed batch rather than halting collection for good.
        let unknown = read_unknown(&self.config.spool);
        if let Some(unknown) = unknown
            && self.unknown_reported != Some(unknown)
        {
            gaps.push(unknown.notice());
        }
        let mut sampled = if include_metrics {
            self.sample_collection_host(&mut gaps)
        } else {
            None
        };
        let (mut metrics, mut updated_history, mut metric_points) =
            self.collection_metrics(sampled.as_ref(), now)?;
        let mut lines = Vec::new();
        let mut pending_cursors = Vec::new();
        let mut remaining = LOG_BODY_BUDGET.min(
            fabric_frame::envelope::MAX_BATCH
                .saturating_sub(metrics.len())
                .saturating_sub(BATCH_RESERVE),
        );
        let mut log_backlog_bytes = 0_u64;
        // The shared per-cycle body budget is spent in path order, starting at a
        // different path each batch, so one busy file cannot starve the others.
        let count = self.config.logs.len();
        let first = if count == 0 {
            0
        } else {
            (self.journal.next_sequence() % count as u64) as usize
        };
        for index in 0..count {
            let path = &self.config.logs[(first + index) % count];
            let name = path.to_string_lossy().to_string();
            let per_line = name.len() + LOG_LINE_OVERHEAD;
            let read = {
                #[cfg(feature = "phase-probe")]
                let _phase = fabric_frame::probe::span("native_file_collection");
                read_log(path, self.cursors.get(&name), remaining, per_line)
            };
            match read {
                Ok(read) => {
                    log_backlog_bytes = log_backlog_bytes.saturating_add(read.backlog_bytes);
                    remaining = remaining.saturating_sub(
                        read.lines
                            .iter()
                            .map(|line| line.body.len() + per_line)
                            .sum::<usize>(),
                    );
                    lines.extend(read.lines);
                    pending_cursors.push(read.cursor);
                    gaps.extend(read.gaps.into_iter().map(bounded_gap));
                }
                Err(error) => {
                    gaps.push(bounded_gap(format!(
                        "log source unavailable {name}: {error}"
                    )));
                    // Carry the committed cursor forward so the newest batch
                    // always holds every configured cursor, even after older
                    // spool files are reclaimed.
                    if let Some(previous) = self.cursors.get(&name) {
                        pending_cursors.push(previous.clone());
                    }
                }
            }
        }
        debug_assert!(gaps.len() <= MAX_GAPS_PER_BATCH);
        if !include_metrics && lines.is_empty() && gaps.is_empty() {
            let cursor_progress = pending_cursors.iter().any(|cursor| {
                let prior = self.cursors.get(&cursor.path);
                let identity_progress = prior.map_or(cursor.btrfs_identity.is_some(), |old| {
                    old.btrfs_identity != cursor.btrfs_identity
                });
                let skip_progress = prior.is_some_and(|old| {
                    old.skipping_oversize
                        && old.path == cursor.path
                        && old.inode == cursor.inode
                        && (old.device == cursor.device
                            || old.btrfs_identity.is_some()
                                && old.btrfs_identity == cursor.btrfs_identity)
                        && cursor.offset > old.offset
                });
                identity_progress || skip_progress
            });
            if !cursor_progress {
                return Ok(None);
            }
            // Cursor-only Batches are invalid. Commit skip progress or stable
            // identity migration with real metrics or the existing host-failure
            // gap. Do not reread the source or synthesize a migration gap.
            sampled = self.sample_collection_host(&mut gaps);
            (metrics, updated_history, metric_points) =
                self.collection_metrics(sampled.as_ref(), now)?;
        }
        debug_assert!(gaps.len() <= MAX_GAPS_PER_BATCH);
        let identity = match sampled.as_ref() {
            Some(s) => Some((s.hostname.clone(), s.boot_id.clone())),
            None => host::identity(&self.host_paths).ok(),
        };
        let (hostname, boot_id) = identity
            .as_ref()
            .map(|(h, b)| (h.as_str(), b.as_str()))
            .unwrap_or(("unknown", "unknown"));
        let log_records = lines.len();
        let logs = if lines.is_empty() {
            Vec::new()
        } else {
            encoded_logs(lines, hostname, boot_id, now)
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
            traces: Vec::new(),
        };
        let accepted_stamp = self.timing_sample();
        let committed = match self.journal.append_owned(candidate) {
            Ok(batch) => batch,
            Err(error) => {
                // Source cursors and counter history remain unadvanced on failure.
                // The candidate is discarded; persist unknown coverage.
                let replace = self.unknown_reported.is_some();
                if let Err(marker) = mark_unknown(&self.config.spool, now, replace) {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("{error}; writing coverage-unknown also failed: {marker}"),
                    ));
                }
                self.unknown_reported = None;
                return Err(error);
            }
        };
        let committed_stamp = self.timing_sample();
        self.timing(committed.sequence, "collection_started", observed_stamp);
        self.timing(committed.sequence, "sources_accepted", accepted_stamp);
        self.timing(committed.sequence, "spool_committed", committed_stamp);
        // Update in-memory state first: the batch is committed, and a caller
        // that retries after a later error must not collect the same lines.
        let sequence = committed.sequence;
        self.meter.record_commit(super::meter::Committed {
            logs: (committed.logs.len() as u64, log_records as u64),
            metrics: (committed.metrics.len() as u64, metric_points as u64),
            traces: (0, 0),
        });
        self.last_backlog = log_backlog_bytes;
        let gap_count = committed.collection_gaps.len();
        for cursor in committed.cursors {
            self.cursors.insert(cursor.path.clone(), cursor);
        }
        if let Some(history) = updated_history {
            self.history = history;
        }
        if let Some(unknown) = unknown {
            // The notice is committed. If removing the marker fails, remember
            // that it was reported and retry the removal next cycle.
            self.unknown_reported = Some(unknown);
            if clear_unknown(&self.config.spool).is_ok() {
                self.unknown_reported = None;
            }
        }
        Ok(Some(Cycle {
            batch_sequence: sequence,
            metric_points,
            log_records,
            gaps: gap_count,
            spool_bytes: self.journal.used_bytes(),
            log_backlog_bytes,
        }))
    }
}

const UNKNOWN_MARKER: &str = "coverage-unknown";
const UNKNOWN_STAGED: &str = "coverage-unknown.tmp";

/// Start of an interval whose collection did not commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unknown {
    Since(u64),
    /// The marker exists but its time cannot be read (torn by an older
    /// binary, or corrupted). Coverage is still unknown.
    UnrecordedTime,
}

impl Unknown {
    fn notice(self) -> String {
        match self {
            Unknown::Since(ns) => {
                format!("coverage unknown since {ns} ns: collection was paused or did not commit")
            }
            Unknown::UnrecordedTime => {
                "coverage unknown since an unrecorded time: collection was paused or did not commit"
                    .to_owned()
            }
        }
    }
}

/// Keep the earliest failed attempt; later failures extend the same interval.
/// `replace` starts a new interval over a marker whose notice was already
/// committed. Written by synced rename so a kill never leaves a torn marker.
fn mark_unknown(dir: &Path, since_ns: u64, replace: bool) -> io::Result<()> {
    let path = dir.join(UNKNOWN_MARKER);
    if path.exists() && !replace {
        return Ok(());
    }
    let staged = dir.join(UNKNOWN_STAGED);
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&staged)?;
    writeln!(file, "{since_ns}")?;
    file.sync_all()?;
    std::fs::rename(&staged, &path)?;
    File::open(dir)?.sync_all()
}

/// Any marker that exists means coverage is unknown, readable or not. A
/// staged marker left by a failed write means the same: `mark_unknown` runs
/// only after a cycle failed to commit.
fn read_unknown(dir: &Path) -> Option<Unknown> {
    let file = match File::open(dir.join(UNKNOWN_MARKER)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return dir
                .join(UNKNOWN_STAGED)
                .exists()
                .then_some(Unknown::UnrecordedTime);
        }
        Err(_) => return Some(Unknown::UnrecordedTime),
    };
    let mut text = String::new();
    match file.take(32).read_to_string(&mut text) {
        Ok(_) => Some(
            text.trim()
                .parse()
                .map_or(Unknown::UnrecordedTime, Unknown::Since),
        ),
        Err(_) => Some(Unknown::UnrecordedTime),
    }
}

/// Called only after the batch carrying the gap notice has committed.
fn clear_unknown(dir: &Path) -> io::Result<()> {
    for name in [UNKNOWN_MARKER, UNKNOWN_STAGED] {
        match std::fs::remove_file(dir.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
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
    /// The next batch sequence the spool will assign.
    pub next_sequence: u64,
    /// Highest sequence the server has durably acknowledged.
    pub acked_through: u64,
    /// Bytes in configured logs after their last committed cursors; a file
    /// with a different identity counts in full, a missing file as zero.
    pub log_backlog_bytes: u64,
}

pub fn inspect(config: &Config) -> io::Result<Report> {
    config.validate()?;
    let mut batches = 0;
    let mut metric_points = 0;
    let mut log_records = 0;
    let mut otlp_payload_bytes = 0_u64;
    let mut gaps = 0;
    let mut last_cursors = BTreeMap::new();
    let info = Spool::inspect(&config.spool, config.journal_cap(), |batch| {
        batches += 1;
        for cursor in &batch.cursors {
            last_cursors.insert(cursor.path.clone(), cursor.clone());
        }
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
    let mut log_backlog_bytes = 0_u64;
    for path in &config.logs {
        let prior = last_cursors.get(path.to_string_lossy().as_ref());
        // A missing or unreadable file contributes nothing it could deliver.
        if let Ok(unread) = log_source::unread_bytes(path, prior) {
            log_backlog_bytes = log_backlog_bytes.saturating_add(unread);
        }
    }
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
        next_sequence: info.next_sequence,
        acked_through: info.acked_through,
        log_backlog_bytes,
    })
}

#[cfg(test)]
#[path = "skip_progress_tests.rs"]
mod skip_progress_tests;

#[cfg(test)]
#[path = "runtime_ownership_tests.rs"]
mod runtime_ownership_tests;

#[cfg(test)]
#[path = "local_logs_tests.rs"]
mod local_logs_tests;

#[cfg(test)]
#[path = "overlap_tests.rs"]
mod overlap_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_operational_clock_is_unmeasured_without_failing_commit() {
        let stamp = timing_stamp_read(std::time::Instant::now(), || {
            Err(io::Error::other("registered clock-read failure"))
        });
        assert_eq!(stamp.unix_ns, None);
        assert!(stamp.monotonic_after_ns >= stamp.monotonic_before_ns);
    }

    #[test]
    fn timing_clock_conversion_rejects_negative_invalid_and_overflow_values() {
        assert_eq!(checked_clock_ns(-1, 0), None);
        assert_eq!(checked_clock_ns(0, -1), None);
        assert_eq!(checked_clock_ns(0, 1_000_000_000), None);
        assert_eq!(checked_clock_ns(libc::time_t::MAX, 0), None);
        assert_eq!(checked_clock_ns(1, 2), Some(1_000_000_002));
        assert!(boot_monotonic_ns().is_some());
    }

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
            let (metrics, next, _) =
                metric_request(&snapshot, 100 + boot, &history, Vec::new()).unwrap();
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
                    btrfs_identity: None,
                }],
                traces: Vec::new(),
            };
            absorb(batch, &allowed, &mut cursors, &mut recovered).unwrap();
            assert_eq!(recovered.len(), 1);
            assert!(cursors.is_empty());
            history = next;
        }
    }
}
