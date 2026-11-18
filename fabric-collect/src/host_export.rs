use crate::{CollectedState, IngestEnvelope};
use fabric::observation::{Data, MetricType};
use fabric::sdk::{Admission, Client, Config};
use fabric_core::{MetricPoint, SpindleProvenance};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

const NODE_ENV: &str = "FABRIC_HOST_EXPORT_NODE";
const PRODUCER: &str = "fabric.host-export";
const RESOURCE_PREFIX: &str = "host/";
const METRIC_PREFIX: &str = "fabric.host.";
const SAMPLE_INTERVAL: Duration = Duration::from_secs(30);
const SUBMIT_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_SAMPLES: usize = 20;
const SLICES: &[(&str, &str, &str)] = &[
    (
        "app.slice",
        "fabric.host.cgroup.app.cpu_usage_usec",
        "fabric.host.cgroup.app.memory_bytes",
    ),
    (
        "agent.slice",
        "fabric.host.cgroup.agent.cpu_usage_usec",
        "fabric.host.cgroup.agent.memory_bytes",
    ),
    (
        "session.slice",
        "fabric.host.cgroup.session.cpu_usage_usec",
        "fabric.host.cgroup.session.memory_bytes",
    ),
    (
        "fabrico11y.slice",
        "fabric.host.cgroup.fabrico11y.cpu_usage_usec",
        "fabric.host.cgroup.fabrico11y.memory_bytes",
    ),
    (
        "system.slice",
        "fabric.host.cgroup.system.cpu_usage_usec",
        "fabric.host.cgroup.system.memory_bytes",
    ),
];

#[derive(Clone, Copy)]
struct Sample {
    name: &'static str,
    value: f64,
    unit: &'static str,
    kind: MetricType,
}

/// A fixed-size handoff from the resident collector to local durable custody.
pub(crate) struct HostExporter {
    client: Client,
    dropped: u64,
    counters: BTreeMap<&'static str, f64>,
    next_sample: Instant,
}

impl HostExporter {
    pub(crate) fn new(socket: Option<&Path>, node: &str) -> Result<Self, String> {
        let socket = socket.ok_or_else(|| format!("{NODE_ENV} requires a local Spindle socket"))?;
        let mut config = Config::local(socket.to_owned(), PRODUCER.into())?;
        config.resource = format!("{RESOURCE_PREFIX}{node}");
        config.slots = 2 * MAX_SAMPLES;
        config.max_record = 1024;
        config.delivery_timeout = SUBMIT_TIMEOUT;
        Ok(Self {
            client: Client::new(config)?,
            dropped: 0,
            counters: BTreeMap::new(),
            next_sample: Instant::now(),
        })
    }

    fn record(&mut self, sample: Sample, timestamp_ns: u64) {
        let reset = sample.kind == MetricType::Counter
            && self
                .counters
                .get(sample.name)
                .is_none_or(|previous| sample.value < *previous);
        let admission = self.client.try_record(
            Data::Metric {
                name: sample.name.into(),
                unit: sample.unit.into(),
                value: sample.value,
                metric_type: sample.kind,
                reset,
            },
            timestamp_ns,
        );
        if matches!(admission, Admission::Accepted { .. }) {
            if sample.kind == MetricType::Counter {
                self.counters.insert(sample.name, sample.value);
            }
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    pub(crate) fn collect(&mut self, state: &mut CollectedState) {
        let now = Instant::now();
        if now < self.next_sample {
            return;
        }
        self.next_sample = now + SAMPLE_INTERVAL;
        let timestamp_ms = state.snapshot.timestamp_unix_ms;

        if !state.snapshot.self_health.self_shedding_active {
            let timestamp_ns = (timestamp_ms.max(0) as u64).saturating_mul(1_000_000);
            for sample in samples(state) {
                self.record(sample, timestamp_ns);
            }
        }

        let retries = self.client.status().retries;
        for (name, count) in [
            ("fabric.host_export.dropped_total", self.dropped),
            ("fabric.host_export.submit_retries_total", retries),
        ] {
            state.metrics.push(MetricPoint {
                timestamp_ms,
                name: name.to_owned(),
                value: count as f64,
                host: state.snapshot.hostname.clone(),
                slice: "fabrico11y.slice".into(),
                unit: "fabric-daemon.service".into(),
                pid: Some(std::process::id()),
                labels_json: "{}".into(),
            });
        }
    }
}

/// Returns the configured node identity for local host-sample forwarding.
pub fn configured_host_export_node() -> Result<Option<String>, String> {
    match std::env::var(NODE_ENV) {
        Ok(node) if valid_node(&node) => Ok(Some(node)),
        Ok(_) => Err(format!(
            "{NODE_ENV} must contain 1–64 ASCII letters, digits, dots, hyphens, or underscores"
        )),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(err) => Err(format!("invalid {NODE_ENV}: {err}")),
    }
}

fn valid_node(node: &str) -> bool {
    (1..=64).contains(&node.len())
        && node
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn samples(state: &CollectedState) -> Vec<Sample> {
    let snapshot = &state.snapshot;
    let mut samples = Vec::with_capacity(MAX_SAMPLES);
    for &(slice, cpu_name, _) in SLICES {
        if let Some(summary) = snapshot.slices.iter().find(|item| item.slice_name == slice) {
            samples.push(Sample {
                name: cpu_name,
                value: summary.cpu_usage_usec as f64,
                unit: "microseconds",
                kind: MetricType::Counter,
            });
        }
    }
    let mut push = |name, value: f64, unit| {
        if value.is_finite() {
            samples.push(Sample {
                name,
                value,
                unit,
                kind: MetricType::Gauge,
            });
        }
    };
    push(
        "fabric.host.cpu.pressure.some.avg10",
        snapshot.psi.cpu_some.avg10,
        "percent",
    );
    push(
        "fabric.host.memory.available_bytes",
        snapshot.memory.mem_available_bytes as f64,
        "bytes",
    );
    push(
        "fabric.host.swap.used_bytes",
        snapshot.memory.swap_used_bytes as f64,
        "bytes",
    );
    push(
        "fabric.host.memory.pressure.some.avg10",
        snapshot.psi.memory_some.avg10,
        "percent",
    );
    push(
        "fabric.host.io.pressure.some.avg10",
        snapshot.psi.io_some.avg10,
        "percent",
    );

    for &(slice, _, memory_name) in SLICES {
        if let Some(summary) = snapshot.slices.iter().find(|item| item.slice_name == slice) {
            push(memory_name, summary.memory_current_bytes as f64, "bytes");
        }
    }

    push(
        "fabric.host.systemd.observed_active_units",
        snapshot
            .units
            .iter()
            .filter(|unit| unit.active_state == "active")
            .count() as f64,
        "units",
    );
    push(
        "fabric.host.systemd.observed_failed_units",
        snapshot
            .units
            .iter()
            .filter(|unit| unit.active_state == "failed")
            .count() as f64,
        "units",
    );
    push(
        "fabric.host.systemd.observed_transient_without_collect",
        snapshot
            .units
            .iter()
            .filter(|unit| unit.transient && !unit.collect_mode_enabled)
            .count() as f64,
        "units",
    );
    push(
        "fabric.host.filesystem.observed_paths",
        snapshot.fs_churn.len() as f64,
        "paths",
    );
    let observed_churn = snapshot.fs_churn.iter().fold(0u64, |total, entry| {
        total.saturating_add(entry.recent_delta_bytes.unsigned_abs())
    });
    push(
        "fabric.host.filesystem.observed_churn_bytes",
        observed_churn as f64,
        "bytes",
    );
    samples
}

/// Whether a local Spindle metric repeats a sample for the configured export node.
pub fn is_forwarded_host_signal(
    provenance: &SpindleProvenance,
    envelope: &IngestEnvelope,
    configured_node: Option<&str>,
) -> bool {
    let (IngestEnvelope::Metric(metric), Some(node)) = (envelope, configured_node) else {
        return false;
    };
    provenance.producer == PRODUCER
        && metric.slice == "spindle"
        && metric.name.starts_with(METRIC_PREFIX)
        && metric.unit.strip_prefix(RESOURCE_PREFIX) == Some(node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation_to_telemetry;
    use fabric::observation::{
        Identity, Observation, REGISTRY, StoredObservation, VERSION, decode,
    };
    use fabric::spindle::{Reply, custody::Receipt};
    use fabric_core::{FsChurnEntry, HostSnapshot, SliceSummary, UnitRecord};
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    use std::thread;

    fn accept_payload(listener: &UnixListener) -> (std::os::unix::net::UnixStream, Vec<u8>) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "Spindle submission timed out");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(err) => panic!("{err}"),
            }
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut header = [0; 4];
        socket.read_exact(&mut header).unwrap();
        let size = u32::from_be_bytes(header) as usize;
        assert!(size <= fabric::observation::MAX_FRAME);
        let mut raw = vec![0; size];
        socket.read_exact(&mut raw).unwrap();
        (socket, raw)
    }

    #[test]
    fn samples_are_bounded_and_omit_path_and_unit_names() {
        let state = CollectedState {
            snapshot: HostSnapshot {
                slices: [
                    SLICES
                        .iter()
                        .map(|(name, _, _)| SliceSummary {
                            slice_name: (*name).into(),
                            cpu_usage_usec: 72,
                            memory_current_bytes: 10,
                            ..Default::default()
                        })
                        .collect::<Vec<_>>(),
                    vec![
                        SliceSummary {
                            slice_name: "untrusted.slice".into(),
                            ..Default::default()
                        };
                        200
                    ],
                ]
                .concat(),
                units: vec![UnitRecord {
                    name: "private-worker.service".into(),
                    active_state: "failed".into(),
                    ..Default::default()
                }],
                fs_churn: vec![FsChurnEntry {
                    path: "/private/path".into(),
                    recent_delta_bytes: -42,
                    ..Default::default()
                }],
                ..Default::default()
            },
            drift: Default::default(),
            graph: Default::default(),
            metrics: Vec::new(),
            fs_events: Vec::new(),
        };
        let result = samples(&state);
        assert_eq!(result.len(), 20);
        for (name, value) in [
            ("fabric.host.cgroup.app.cpu_usage_usec", 72.0),
            ("fabric.host.systemd.observed_failed_units", 1.0),
            ("fabric.host.filesystem.observed_churn_bytes", 42.0),
        ] {
            assert!(
                result
                    .iter()
                    .any(|sample| sample.name == name && sample.value == value)
            );
        }
        assert!(result.iter().all(|sample| !sample.name.contains("private")));
    }

    #[test]
    fn only_the_marked_origin_copy_is_excluded_from_local_projection() {
        let mut observation = Observation {
            version: VERSION,
            registry: REGISTRY.into(),
            identity: Identity {
                producer: PRODUCER.into(),
                generation: "generation".into(),
                sequence: 1,
            },
            timestamp_ns: 1_000_000,
            resource: "host/node-a".into(),
            data: Data::Metric {
                name: "fabric.host.memory.available_bytes".into(),
                unit: "bytes".into(),
                value: 1.0,
                metric_type: MetricType::Gauge,
                reset: false,
            },
        };
        let exported =
            observation_to_telemetry(&StoredObservation::V2(observation.clone()), "node-a");
        let mut key = SpindleProvenance {
            origin: "node-a-origin".into(),
            record_id: 1,
            producer: observation.identity.producer.clone(),
            generation: observation.identity.generation.clone(),
            sequence: 1,
        };
        assert!(is_forwarded_host_signal(&key, &exported, Some("node-a")));
        assert!(!is_forwarded_host_signal(&key, &exported, Some("node-b")));
        assert!(!is_forwarded_host_signal(&key, &exported, None));
        observation.identity.producer = "application".into();
        key.producer = "application".into();
        let application = observation_to_telemetry(&StoredObservation::V2(observation), "node-a");
        assert!(!is_forwarded_host_signal(
            &key,
            &application,
            Some("node-a")
        ));
    }

    #[test]
    fn uncertain_submission_reuses_the_exact_identity_and_bytes() {
        let tmp = fabric_store::validate_non_tmp_path_on_disk(Path::new(
            &std::env::var("TMPDIR").unwrap_or_else(|_| "/data/tmp".into()),
        ))
        .unwrap();
        let dir = tempfile::tempdir_in(tmp).unwrap();
        let path = dir.path().join("spindle.sock");
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut exporter = HostExporter::new(Some(&path), "node-b").unwrap();
        let sample = Sample {
            name: "fabric.host.cgroup.app.cpu_usage_usec",
            value: 10.0,
            unit: "microseconds",
            kind: MetricType::Counter,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while exporter.counters.is_empty() {
            exporter.record(sample, 100_000_000);
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        let (first, first_raw) = accept_payload(&listener);
        drop(first);
        let (mut second, second_raw) = accept_payload(&listener);
        assert_eq!(first_raw, second_raw);
        let decoded = decode(&second_raw).unwrap();
        assert_eq!(decoded.identity.producer, PRODUCER);
        assert_eq!(decoded.identity.sequence, 1);
        assert_eq!(decoded.resource, "host/node-b");
        assert!(matches!(decoded.data, Data::Metric { reset: true, .. }));
        let reply = serde_json::to_vec(&Reply::Receipt {
            receipt: Receipt {
                id: 1,
                identity: decoded.identity,
                duplicate: true,
            },
        })
        .unwrap();
        second
            .write_all(&(reply.len() as u32).to_be_bytes())
            .unwrap();
        second.write_all(&reply).unwrap();
        assert_eq!(exporter.client.status().retries, 1);
    }
}
