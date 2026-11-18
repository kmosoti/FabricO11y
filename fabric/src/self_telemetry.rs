//! Bounded service telemetry delivery to a dedicated local Spindle store.

use crate::{
    observation::{self, Data},
    sdk::{self, Admission},
    spindle::custody::Receipt,
};
use serde::Serialize;
use std::{
    fs,
    io::ErrorKind,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

pub const SOCKET_ENV: &str = "FABRIC_SELF_TELEMETRY_SOCKET";
pub const MAX_QUEUED_RECORDS: usize = 32;
pub const MAX_RECORD_BYTES: usize = 4096;
const IO_BUDGET: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    Spindle,
    FabricServer,
}

impl Service {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Spindle => "spindle",
            Self::FabricServer => "fabric-server",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub dropped: u64,
    pub delivery: sdk::Status,
}

/// Owns one SDK delivery worker and a fixed payload pool. Accepted slots become
/// durable when Spindle issues matching receipts. Queue drops are local and do
/// not consume a Spindle record ID.
pub struct SelfTelemetry {
    client: sdk::Client,
    dropped: AtomicU64,
}

impl SelfTelemetry {
    pub fn from_env(
        service: Service,
        exclude_socket: Option<&Path>,
    ) -> Result<Option<Self>, String> {
        let Some(socket) = std::env::var_os(SOCKET_ENV).filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        Self::open(service, Path::new(&socket), exclude_socket).map(Some)
    }

    pub fn open(
        service: Service,
        socket: &Path,
        exclude_socket: Option<&Path>,
    ) -> Result<Self, String> {
        let socket = private_socket_path(socket)?;
        if let Some(excluded) = exclude_socket
            && socket == private_socket_path(excluded)?
        {
            return Err("self-telemetry Spindle socket must differ from the service socket".into());
        }
        let name = service.name();
        let mut config = sdk::Config::local(socket, format!("{name}.self"))?;
        config.resource = format!("{name}/pid/{}", std::process::id());
        config.slots = MAX_QUEUED_RECORDS;
        config.max_record = MAX_RECORD_BYTES;
        config.delivery_timeout = IO_BUDGET;
        Ok(Self {
            client: sdk::Client::new(config)?,
            dropped: AtomicU64::new(0),
        })
    }

    /// Admits a telemetry record without blocking on the worker or Spindle.
    pub fn try_emit(&self, data: Data) -> Admission {
        let result = self.client.try_record(data, observation::now_ns());
        if !matches!(result, Admission::Accepted { .. }) {
            let _ = self
                .dropped
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                    Some(count.saturating_add(1))
                });
        }
        result
    }

    pub fn status(&self) -> Status {
        Status {
            dropped: self.dropped.load(Ordering::Relaxed),
            delivery: self.client.status(),
        }
    }

    /// Optional bounded drain during service shutdown, outside the ingest path.
    pub fn flush_local(&self, deadline: Instant) -> Result<Option<Receipt>, String> {
        self.client.flush(deadline)
    }
}

fn private_socket_path(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return Err(
            "self-telemetry socket must have an absolute path without parent traversal".into(),
        );
    }
    let name = path
        .file_name()
        .ok_or("missing self-telemetry socket name")?;
    let parent = path
        .parent()
        .ok_or("missing self-telemetry socket parent")?
        .canonicalize()
        .map_err(|error| format!("resolve self-telemetry socket directory: {error}"))?;
    if [path, parent.as_path()].into_iter().any(|path| {
        ["/tmp", "/dev/shm", "/run/shm"]
            .iter()
            .any(|base| path.starts_with(base))
    }) {
        return Err("self-telemetry socket directory cannot be a RAM scratch path".into());
    }
    let meta = fs::metadata(&parent).map_err(|error| error.to_string())?;
    let uid = rustix::process::getuid().as_raw();
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
        return Err("self-telemetry socket directory must be private and owned".into());
    }
    let socket = parent.join(name);
    match fs::symlink_metadata(&socket) {
        Ok(meta)
            if meta.file_type().is_socket() && meta.uid() == uid && meta.mode() & 0o077 == 0 => {}
        Ok(_) => return Err("self-telemetry socket must be a private owned Unix socket".into()),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    Ok(socket)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::{PermissionsExt, symlink},
        thread,
    };

    fn private_temp() -> tempfile::TempDir {
        tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap()
    }

    fn emit_after_contention(telemetry: &SelfTelemetry, data: Data) -> Admission {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let admitted = telemetry.try_emit(data.clone());
            if admitted != Admission::Contended || Instant::now() >= deadline {
                return admitted;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn rejects_feedback_socket_and_counts_bounded_admission() {
        let dir = private_temp();
        let alias = dir.path().join("alias");
        symlink(dir.path(), &alias).unwrap();
        let socket = dir.path().join("telemetry.sock");
        assert!(
            SelfTelemetry::open(
                Service::Spindle,
                &socket,
                Some(&alias.join("telemetry.sock"))
            )
            .is_err()
        );
        let client = SelfTelemetry::open(Service::FabricServer, &socket, None).unwrap();
        let too_wide = Data::Event {
            name: "event".into(),
            message: "x".repeat(MAX_RECORD_BYTES),
        };
        let mut dropped = 1;
        loop {
            let admission = client.try_emit(too_wide.clone());
            if admission != Admission::Contended {
                assert_eq!(admission, Admission::TooLarge);
                break;
            }
            dropped += 1;
            assert!(dropped <= 5);
            thread::sleep(Duration::from_millis(10));
        }
        for _ in 0..(MAX_QUEUED_RECORDS * 2) {
            let _ = client.try_emit(Data::Event {
                name: "event".into(),
                message: "test".into(),
            });
        }
        let status = client.status();
        assert_eq!(
            status.delivery.reserved_payload_bytes,
            MAX_QUEUED_RECORDS * MAX_RECORD_BYTES
        );
        assert!(status.delivery.charged_slots <= MAX_QUEUED_RECORDS);
        assert_eq!(status.delivery.confirmed, 0);
        assert_eq!(
            status.dropped + status.delivery.accepted,
            (MAX_QUEUED_RECORDS * 2 + dropped) as u64
        );
    }

    #[test]
    fn service_signals_reach_distinct_local_spindle_custody() {
        use crate::{
            observation::{Level, MetricType, Outcome},
            spindle::{Config, Daemon, Reply, Request, transport},
        };
        let dir = private_temp();
        let socket = dir.path().join("telemetry.sock");
        let config: Config = serde_json::from_value(serde_json::json!({
            "socket": socket,
            "storage": dir.path().join("custody"),
            "quota_bytes": 2 * 1024 * 1024,
        }))
        .unwrap();
        let mut daemon = Daemon::open(config).unwrap();
        let worker = thread::spawn(move || daemon.run());
        let spindle = SelfTelemetry::open(
            Service::Spindle,
            &socket,
            Some(&dir.path().join("workload.sock")),
        )
        .unwrap();
        let server = SelfTelemetry::open(Service::FabricServer, &socket, None).unwrap();
        for (source, data) in [
            (
                &spindle,
                Data::Metric {
                    name: "backlog".into(),
                    unit: "records".into(),
                    value: 2.0,
                    metric_type: MetricType::Gauge,
                    reset: false,
                },
            ),
            (
                &spindle,
                Data::Event {
                    name: "ready".into(),
                    message: "ready".into(),
                },
            ),
            (
                &server,
                Data::Log {
                    level: Level::Info,
                    message: "ready".into(),
                },
            ),
            (
                &server,
                Data::ServiceRequest {
                    service: "fabric-server".into(),
                    operation: "query".into(),
                    duration_ns: 1,
                    status: Outcome::Ok,
                    trace_id: None,
                },
            ),
        ] {
            assert!(matches!(
                emit_after_contention(source, data),
                Admission::Accepted { .. }
            ));
        }
        let deadline = Instant::now() + Duration::from_secs(4);
        let spindle_receipt = spindle.flush_local(deadline).unwrap().unwrap();
        let server_receipt = server.flush_local(deadline).unwrap().unwrap();
        assert_eq!(spindle_receipt.identity.producer, "spindle.self");
        assert_eq!(server_receipt.identity.producer, "fabric-server.self");
        assert_ne!(
            spindle_receipt.identity.generation,
            server_receipt.identity.generation
        );
        let mut retained = Vec::new();
        for fence in 0..4 {
            let records = transport::read_raw(&socket, fence, deadline).unwrap();
            assert_eq!(records[0].0, fence + 1);
            retained.push(observation::decode_versioned(&records[0].1).unwrap());
        }
        assert_eq!(retained.len(), 4);
        for expected in ["metric", "event", "log", "service_request"] {
            assert_eq!(
                retained
                    .iter()
                    .filter(
                        |record| serde_json::to_value(record.data()).unwrap()["kind"] == expected
                    )
                    .count(),
                1
            );
        }
        assert!(matches!(
            transport::request(&socket, &Request::Stop, deadline),
            Ok(Reply::Stopped)
        ));
        worker.join().unwrap().unwrap();
    }
}
