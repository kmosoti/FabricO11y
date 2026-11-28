//! Registry observations about receiver operations, exported outside storage locks.

use super::Auth;
use fabric::{
    canonical::Request,
    observation::{Data, Level, MetricType, Outcome},
    self_telemetry::{SelfTelemetry, Service},
};
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
    time::{Duration, Instant},
};

pub(super) const REPORT_INTERVAL: Duration = Duration::from_secs(60);
const ORIGINS_ENV: &str = "FABRIC_SELF_TELEMETRY_ORIGINS";

struct Scope {
    telemetry_origins: BTreeSet<String>,
}

impl Scope {
    fn new(value: &str, auth: &Auth) -> Result<Self, String> {
        let mut telemetry_origins = BTreeSet::new();
        for origin in value.split(',').map(str::trim) {
            if origin.is_empty() || !auth.nodes.contains_key(origin) {
                return Err(format!("{ORIGINS_ENV} must list existing telemetry grants"));
            }
            if !telemetry_origins.insert(origin.to_owned()) {
                return Err(format!("{ORIGINS_ENV} contains a duplicate grant"));
            }
        }
        Ok(Self { telemetry_origins })
    }

    fn excludes(&self, origin: &str) -> bool {
        self.telemetry_origins.contains(origin)
    }

    fn classify(
        &self,
        request: &Request,
        node: Option<&str>,
        reader: bool,
        auth: &Auth,
    ) -> Option<Operation> {
        match request {
            Request::Ingest { origin, .. }
                if node == Some(origin.as_str()) && !self.excludes(origin) =>
            {
                Some(Operation::Commit)
            }
            Request::Query { request }
                if reader
                    && !request.scope.is_empty()
                    && request.scope.iter().all(|origin| {
                        auth.nodes.contains_key(origin) && !self.excludes(origin)
                    }) =>
            {
                Some(Operation::Query)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
enum Operation {
    Commit,
    Query,
}

impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Commit => "commit",
            Self::Query => "query",
        }
    }
}

#[derive(Default)]
struct OperationStats {
    completed: AtomicU64,
    failed: AtomicU64,
    last: AtomicU64,
}

impl OperationStats {
    fn record(&self, duration: Duration, succeeded: bool) {
        // Keep the outcome and its duration together even with concurrent workers.
        let duration_ns = duration.as_nanos().min((u64::MAX >> 1) as u128) as u64;
        self.last
            .store((duration_ns << 1) | u64::from(succeeded), Relaxed);
        if !succeeded {
            self.failed.fetch_add(1, Relaxed);
        }
        self.completed.fetch_add(1, Relaxed);
    }

    fn report(&self, telemetry: &SelfTelemetry, operation: Operation, previous: &mut u64) {
        let completed = self.completed.load(Relaxed);
        let name = operation.name();
        metric(
            telemetry,
            &format!("fabric.server.{name}.requests"),
            completed,
            MetricType::Counter,
        );
        metric(
            telemetry,
            &format!("fabric.server.{name}.failures"),
            self.failed.load(Relaxed),
            MetricType::Counter,
        );
        if completed != *previous {
            *previous = completed;
            let last = self.last.load(Relaxed);
            metric_with_unit(
                telemetry,
                &format!("fabric.server.{name}.last_duration"),
                last >> 1,
                MetricType::Gauge,
                "ns",
            );
            let _ = telemetry.try_emit(Data::ServiceRequest {
                service: Service::FabricServer.name().into(),
                operation: name.into(),
                duration_ns: last >> 1,
                status: if last & 1 == 1 {
                    Outcome::Ok
                } else {
                    Outcome::Error
                },
                trace_id: None,
            });
        }
    }
}

#[derive(Default)]
pub(super) struct ReportState {
    prior_commits: u64,
    prior_queries: u64,
    projection_fault: bool,
    exporter_fault: bool,
    reported_drops: u64,
}

pub(super) struct ServerTelemetry {
    telemetry: SelfTelemetry,
    scope: Scope,
    commits: OperationStats,
    queries: OperationStats,
    projected: AtomicU64,
    projection_failures: AtomicU64,
    projection_stopped_with_workload: AtomicBool,
}

impl ServerTelemetry {
    pub(super) fn from_env(auth: &Auth) -> Result<Option<Self>, String> {
        if !std::env::var_os(fabric::self_telemetry::SOCKET_ENV).is_some_and(|v| !v.is_empty()) {
            return Ok(None);
        }
        let origins = std::env::var(ORIGINS_ENV)
            .map_err(|_| format!("{ORIGINS_ENV} is required for receiver telemetry"))?;
        let scope = Scope::new(&origins, auth)?;
        Ok(SelfTelemetry::from_env(Service::FabricServer, None)?
            .map(|telemetry| Self::new(telemetry, scope)))
    }

    fn new(telemetry: SelfTelemetry, scope: Scope) -> Self {
        Self {
            telemetry,
            scope,
            commits: OperationStats::default(),
            queries: OperationStats::default(),
            projected: AtomicU64::new(0),
            projection_failures: AtomicU64::new(0),
            projection_stopped_with_workload: AtomicBool::new(false),
        }
    }

    pub(super) fn measure(
        &self,
        request: &Request,
        node: Option<&str>,
        reader: bool,
        auth: &Auth,
    ) -> Option<Measurement<'_>> {
        let stats = match self.scope.classify(request, node, reader, auth)? {
            Operation::Commit => &self.commits,
            Operation::Query => &self.queries,
        };
        Some(Measurement {
            stats,
            started: Instant::now(),
            succeeded: false,
        })
    }

    pub(super) fn tracks(&self, origin: &str) -> bool {
        !self.scope.excludes(origin)
    }

    pub(super) fn projection(&self, rows: usize, succeeded: bool, retry: bool) {
        if !succeeded && !retry {
            self.projection_stopped_with_workload
                .store(rows > 0, Relaxed);
        }
        if rows > 0 {
            if succeeded {
                self.projected.fetch_add(rows as u64, Relaxed);
            } else {
                self.projection_failures.fetch_add(1, Relaxed);
            }
        }
    }

    pub(super) fn started(&self) {
        self.event("fabric.server.started", "receiver started");
    }

    pub(super) fn report(
        &self,
        state: &mut ReportState,
        projection_lag: Option<u64>,
        projection_fault: Option<bool>,
    ) {
        let telemetry = &self.telemetry;
        self.commits
            .report(telemetry, Operation::Commit, &mut state.prior_commits);
        self.queries
            .report(telemetry, Operation::Query, &mut state.prior_queries);
        metric(
            telemetry,
            "fabric.server.projection.rows",
            self.projected.load(Relaxed),
            MetricType::Counter,
        );
        let failures = self.projection_failures.load(Relaxed);
        metric(
            telemetry,
            "fabric.server.projection.failures",
            failures,
            MetricType::Counter,
        );
        if let Some(lag) = projection_lag {
            metric(
                telemetry,
                "fabric.server.projection.lag",
                lag,
                MetricType::Gauge,
            );
        }
        // A telemetry-only failed batch must not produce an event about itself.
        if let Some(projection_fault) = projection_fault
            .map(|fault| fault && self.projection_stopped_with_workload.load(Relaxed))
            && projection_fault != state.projection_fault
        {
            self.event(
                "fabric.server.projection.state",
                if projection_fault {
                    "projection stopped after workload failure"
                } else {
                    "projection recovered"
                },
            );
            state.projection_fault = projection_fault;
        }
        let status = telemetry.status();
        let exporter_fault = status.delivery.last_error.is_some();
        if (exporter_fault && !state.exporter_fault) || status.dropped != state.reported_drops {
            // Keep outage diagnostics available without an exporter round trip.
            eprintln!("fabric-server: self-telemetry delivery blocked or dropped records");
        }
        state.exporter_fault = exporter_fault;
        state.reported_drops = status.dropped;
        metric(
            telemetry,
            "fabric.server.telemetry.dropped",
            status.dropped,
            MetricType::Counter,
        );
        metric(
            telemetry,
            "fabric.server.telemetry.pending",
            status.delivery.charged_slots as u64,
            MetricType::Gauge,
        );
    }

    fn event(&self, name: &str, message: &str) {
        let _ = self.telemetry.try_emit(Data::Event {
            name: name.into(),
            message: message.into(),
        });
        let _ = self.telemetry.try_emit(Data::Log {
            level: Level::Info,
            message: message.into(),
        });
    }

    pub(super) fn shutdown(&self) {
        let _ = self
            .telemetry
            .flush_local(Instant::now() + Duration::from_millis(250));
    }
}

pub(super) struct Measurement<'a> {
    stats: &'a OperationStats,
    started: Instant,
    succeeded: bool,
}

impl Measurement<'_> {
    pub(super) fn succeed(&mut self) {
        self.succeeded = true;
    }
}

impl Drop for Measurement<'_> {
    fn drop(&mut self) {
        self.stats.record(self.started.elapsed(), self.succeeded);
    }
}

fn metric(telemetry: &SelfTelemetry, name: &str, value: u64, metric_type: MetricType) {
    metric_with_unit(telemetry, name, value, metric_type, "1");
}

fn metric_with_unit(
    telemetry: &SelfTelemetry,
    name: &str,
    value: u64,
    metric_type: MetricType,
    unit: &str,
) {
    let _ = telemetry.try_emit(Data::Metric {
        name: name.into(),
        unit: unit.into(),
        value: value as f64,
        metric_type,
        reset: false,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric::query::QueryRequest;

    fn grants() -> Auth {
        Auth {
            reader: "reader".into(),
            writers: Default::default(),
            writer_bindings: Default::default(),
            nodes: [
                ("service-telemetry".into(), "separate-telemetry-key".into()),
                ("workload".into(), "node-key".into()),
            ]
            .into_iter()
            .collect(),
        }
    }

    #[test]
    fn reporter_emits_receiver_melt_to_spindle_custody() {
        use fabric::{
            observation::decode_versioned,
            spindle::{self, transport},
        };
        use std::{
            fs::Permissions,
            os::unix::fs::PermissionsExt,
            path::{Path, PathBuf},
            thread,
        };

        struct SpindleProcess {
            socket: PathBuf,
            worker: Option<thread::JoinHandle<Result<(), String>>>,
        }

        impl Drop for SpindleProcess {
            fn drop(&mut self) {
                if transport::request(&self.socket, &spindle::Request::Stop, deadline()).is_ok()
                    && let Some(worker) = self.worker.take()
                {
                    let _ = worker.join();
                }
            }
        }

        fn deadline() -> Instant {
            Instant::now() + Duration::from_secs(2)
        }

        let tmp =
            std::env::var_os("TMPDIR").map_or_else(|| PathBuf::from("/data/tmp"), PathBuf::from);
        assert!(
            ![Path::new("/tmp"), Path::new("/dev/shm")]
                .iter()
                .any(|path| tmp.starts_with(path))
        );
        let directory = tempfile::Builder::new()
            .prefix("melt")
            .permissions(Permissions::from_mode(0o700))
            .tempdir_in(tmp)
            .unwrap();
        let socket = directory.path().join("s");
        let config = serde_json::from_value(serde_json::json!({
            "socket": socket,
            "storage": directory.path().join("custody"),
            "quota_bytes": 2 * 1024 * 1024
        }))
        .unwrap();
        let mut daemon = spindle::Daemon::open(config).unwrap();
        let _running = SpindleProcess {
            socket: socket.clone(),
            worker: Some(thread::spawn(move || daemon.run())),
        };
        let auth = grants();
        let telemetry = ServerTelemetry::new(
            SelfTelemetry::open(Service::FabricServer, &socket, None).unwrap(),
            Scope::new("service-telemetry", &auth).unwrap(),
        );
        let mut state = ReportState::default();
        let mut cursor = 0;
        let mut seen = [false; 4];
        for _ in 0..6 {
            // The emitter permits contention drops. Retry a fresh sample with a
            // new workload operation, rather than resending the same report.
            telemetry.started();
            telemetry
                .measure(
                    &Request::Ingest {
                        origin: "workload".into(),
                        records: Vec::new(),
                    },
                    Some("workload"),
                    false,
                    &auth,
                )
                .unwrap()
                .succeed();
            telemetry.projection(3, true, false);
            telemetry.report(&mut state, Some(2), Some(false));
            telemetry.telemetry.flush_local(deadline()).unwrap();
            while let Some((id, raw)) = transport::read_raw(&socket, cursor, deadline())
                .unwrap()
                .into_iter()
                .next()
            {
                assert_eq!(id, cursor + 1);
                cursor = id;
                let record = decode_versioned(&raw).unwrap();
                assert_eq!(record.identity().producer, "fabric-server.self");
                match record.data() {
                    Data::Metric { name, .. } if name.starts_with("fabric.server.") => {
                        seen[0] = true
                    }
                    Data::Event { name, .. } if name == "fabric.server.started" => seen[1] = true,
                    Data::Log { message, .. } if message == "receiver started" => seen[2] = true,
                    Data::ServiceRequest {
                        service,
                        operation,
                        status: Outcome::Ok,
                        ..
                    } if service == "fabric-server" && operation == "commit" => seen[3] = true,
                    _ => {}
                }
            }
            if seen.into_iter().all(|signal| signal) {
                return;
            }
        }
        panic!("missing metrics, events, logs, or traces: {seen:?}");
    }

    #[test]
    fn excludes_telemetry_and_unverified_origins_from_measurements() {
        let auth = grants();
        assert!(Scope::new("unknown", &auth).is_err());
        assert!(Scope::new("workload,workload", &auth).is_err());
        let scope = Scope::new("service-telemetry", &auth).unwrap();
        for (origin, node, allowed) in [
            ("service-telemetry", Some("service-telemetry"), false),
            ("workload", Some("service-telemetry"), false),
            ("workload", Some("workload"), true),
        ] {
            let request = Request::Ingest {
                origin: origin.into(),
                records: Vec::new(),
            };
            assert_eq!(
                scope.classify(&request, node, false, &auth).is_some(),
                allowed
            );
        }
        for (origins, reader, allowed) in [
            (vec!["service-telemetry"], true, false),
            (vec!["workload", "service-telemetry"], true, false),
            (vec!["workload"], false, false),
            (vec![], true, false),
            (vec!["workload"], true, true),
        ] {
            let request: QueryRequest = serde_json::from_value(serde_json::json!({
                "version": 1,
                "query_id": "scope",
                "scope": origins,
                "snapshot": {},
                "filter": {},
                "operation": {"operation": "list_runs"},
                "cursor": null
            }))
            .unwrap();
            assert_eq!(
                scope
                    .classify(&Request::Query { request }, None, reader, &auth)
                    .is_some(),
                allowed
            );
        }
    }
}
