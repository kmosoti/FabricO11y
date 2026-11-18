//! Bounded authoritative receiver and isolated analytical projection worker.
use admission_kernel::telemetry::{elastic_can_admit, elastic_lease_return};

pub mod engine;
pub mod ledger;
mod self_observation;
use fabric::{
    canonical::{
        self, Authenticated, ProjectionReadiness, ProjectionReadyRequest, ProjectionReason,
        ProjectionStatus, Request, Response,
    },
    observation,
    query::QueryRequest,
    resources,
};
use fabric_query::Projection;
use ledger::Ledger;
use parking_lot::{FairMutex, FairMutexGuard};
use self_observation::{REPORT_INTERVAL, ReportState, ServerTelemetry};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    mem::DropGuard,
    net::{SocketAddr, TcpListener},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock, TryLockError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeGrant {
    pub origin: String,
    pub credential_file: PathBuf,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub listen: String,
    pub storage: PathBuf,
    pub projection: PathBuf,
    pub quota_bytes: u64,
    pub projection_quota_bytes: u64,
    pub reader_credential_file: PathBuf,
    pub nodes: Vec<NodeGrant>,
    /// Optional retained payload limits for dedicated telemetry origins.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub node_quota_bytes: BTreeMap<String, u64>,
    #[serde(default)]
    pub engine_endpoint: Option<String>,
    #[serde(default = "connections")]
    pub max_connections: usize,
}
fn connections() -> usize {
    resources::DEFAULT_RECEIVER_CONNECTIONS
}
fn secret(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let m = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if m.file_type().is_symlink()
        || !m.is_file()
        || m.uid() != rustix::process::getuid().as_raw()
        || m.permissions().mode() & 0o077 != 0
        || m.len() > 256
    {
        return Err("credential must be a private owned regular file <=256 bytes".into());
    }
    let mut s = String::new();
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(257)
        .read_to_string(&mut s)
        .map_err(|e| e.to_string())?;
    let s = s.trim().to_owned();
    if s.len() < 16 || s.len() > 256 || !s.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("invalid credential format".into());
    }
    Ok(s)
}
fn matches(a: &str, b: &str) -> bool {
    let a = Sha256::digest(a.as_bytes());
    let b = Sha256::digest(b.as_bytes());
    a.iter().zip(b).fold(0u8, |x, (a, b)| x | (a ^ b)) == 0
}
struct Auth {
    reader: String,
    nodes: BTreeMap<String, String>,
}
struct QuerySlot {
    active: bool,
    id: String,
    cancel: Arc<AtomicBool>,
}
struct QueryGuard(Arc<Mutex<QuerySlot>>);
impl Drop for QueryGuard {
    fn drop(&mut self) {
        if let Ok(mut s) = self.0.lock() {
            s.active = false;
            s.id.clear();
        }
    }
}
struct ReadState {
    pressure: AtomicBool,
    queries: Arc<Mutex<QuerySlot>>,
    admission: FairMutex<()>,
    reader: Mutex<ledger::Reader>,
    authority: String,
    frontiers: RwLock<BTreeMap<String, u64>>,
    engine: Option<engine::Greptime>,
    telemetry: Option<ServerTelemetry>,
}
pub struct Server {
    listener: TcpListener,
    ledger: Arc<Mutex<Ledger>>,
    projection: Arc<Mutex<Projection>>,
    auth: Arc<Auth>,
    stop: Arc<AtomicBool>,
    max_connections: usize,
    projection_error: Arc<Mutex<Option<String>>>,
    reads: Arc<ReadState>,
}
impl Server {
    pub fn open(config: Config) -> Result<Self, String> {
        let address: SocketAddr = config
            .listen
            .parse()
            .map_err(|_| "invalid listen address")?;
        if !address.ip().is_loopback()
            || config.max_connections == 0
            || config.max_connections > 32
            || config.nodes.is_empty()
            || config.nodes.len() > 32
            || !config.storage.is_absolute()
            || !config.projection.is_absolute()
        {
            return Err("invalid receiver limits/paths".into());
        }
        let reader = secret(&config.reader_credential_file)?;
        let mut nodes = BTreeMap::new();
        for node in &config.nodes {
            if node.origin.is_empty() || node.origin.len() > 128 || nodes.contains_key(&node.origin)
            {
                return Err("invalid/duplicate node origin".into());
            }
            let token = secret(&node.credential_file)?;
            if matches(&reader, &token) || nodes.values().any(|s: &String| matches(s, &token)) {
                return Err("credentials must have disjoint grants".into());
            }
            nodes.insert(node.origin.clone(), token);
        }
        if config
            .node_quota_bytes
            .keys()
            .any(|origin| !nodes.contains_key(origin))
        {
            return Err("canonical node quota requires a configured origin".into());
        }
        let ledger = Arc::new(Mutex::new(Ledger::open_with_node_quotas(
            &config.storage,
            config.quota_bytes,
            config.engine_endpoint.as_deref(),
            config.node_quota_bytes,
        )?));
        ledger::private_dir(&config.projection)?;
        let mut projection_state =
            Projection::open(&config.projection, config.projection_quota_bytes)?;
        projection_state.bind_authority(&ledger.lock().map_err(|_| "canonical lock")?.authority)?;
        let frontiers = ledger.lock().map_err(|_| "canonical lock")?.frontiers()?;
        if projection_state
            .progress()
            .frontiers
            .iter()
            .any(|(o, n)| *n > frontiers.get(o).copied().unwrap_or(0))
        {
            return Err("projection ahead of canonical history; rebuild required".into());
        }
        let projection = Arc::new(Mutex::new(projection_state));
        let auth = Auth { reader, nodes };
        let telemetry = ServerTelemetry::from_env(&auth).unwrap_or_else(|error| {
            eprintln!("fabric-server: self-telemetry disabled: {error}");
            None
        });
        let reads = {
            let state = ledger.lock().map_err(|_| "canonical lock")?;
            Arc::new(ReadState {
                pressure: AtomicBool::new(false),
                queries: Arc::new(Mutex::new(QuerySlot {
                    active: false,
                    id: String::new(),
                    cancel: Arc::new(AtomicBool::new(false)),
                })),
                admission: FairMutex::new(()),
                reader: Mutex::new(state.reader()?),
                authority: state.authority.clone(),
                frontiers: RwLock::new(frontiers),
                engine: state.engine(),
                telemetry,
            })
        };
        let listener = TcpListener::bind(address).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        Ok(Self {
            listener,
            ledger,
            projection,
            auth: Arc::new(auth),
            stop: Arc::new(AtomicBool::new(false)),
            max_connections: config.max_connections,
            projection_error: Arc::new(Mutex::new(None)),
            reads,
        })
    }
    pub fn address(&self) -> String {
        self.listener
            .local_addr()
            .expect("bound listener")
            .to_string()
    }
    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }
    pub fn serve(self) -> Result<(), String> {
        let connection_limit = resources::connection_limit(self.max_connections);
        self.serve_with_limit(connection_limit)
    }

    fn serve_with_limit(self, connection_limit: usize) -> Result<(), String> {
        let mut next_sample = self.sample_pressure();
        let mut self_report = ReportState::default();
        let mut next_report = Instant::now();
        if let Some(telemetry) = &self.reads.telemetry {
            telemetry.started();
        }
        let reads = self.reads.clone();
        let projection = self.projection.clone();
        let stop = self.stop.clone();
        let projection_error = self.projection_error.clone();
        let native = reads.engine.is_some();
        let replay_ledger = self.ledger.clone();
        let projector = thread::spawn(move || {
            let result = (|| {
                let mut last = Instant::now() - Duration::from_secs(2);
                while !stop.load(Ordering::SeqCst) {
                    if native
                        || last.elapsed() < Duration::from_secs(1)
                        || !optional_admission(reads.pressure.load(Ordering::Acquire), false)
                    {
                        thread::sleep(Duration::from_millis(25));
                        continue;
                    }
                    let checkpoint = projection
                        .lock()
                        .map_err(|_| "projection lock")?
                        .progress()
                        .checkpoint;
                    // Baseline SQLite work shares one connection. This guard ends
                    // before Parquet publication; native HTTP uses separate reads.
                    let rows = match replay_ledger.lock().map_err(|_| "canonical lock")?.replay(
                        checkpoint,
                        8192,
                        8 * 1024 * 1024,
                    ) {
                        Ok(rows) => rows,
                        Err(err) if retry_projector_pressure(&err) => {
                            thread::sleep(Duration::from_millis(25));
                            continue;
                        }
                        Err(err) => return Err(err),
                    };
                    let full = rows.len() == 8192;
                    if !rows.is_empty() {
                        let result = projection
                            .lock()
                            .map_err(|_| "projection lock")?
                            .apply(&rows);
                        let retry = result
                            .as_ref()
                            .is_err_and(|err| retry_projector_pressure(err));
                        if let Some(telemetry) = &reads.telemetry {
                            let workload_rows = rows
                                .iter()
                                .filter(|row| telemetry.tracks(&row.origin))
                                .count();
                            telemetry.projection(workload_rows, result.is_ok(), retry);
                        }
                        if let Err(err) = result {
                            if retry {
                                thread::sleep(Duration::from_millis(25));
                                continue;
                            }
                            return Err(err);
                        }
                    }
                    if !full {
                        last = Instant::now();
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                Ok::<(), String>(())
            })();
            if let Err(error) = &result
                && let Ok(mut fault) = projection_error.lock()
            {
                *fault = Some(error.clone());
            }
            result
        });
        let active = Arc::new(AtomicUsize::new(0));
        let mut workers = Vec::<thread::JoinHandle<()>>::new();
        let mut failure = None;
        while !self.stop.load(Ordering::SeqCst) {
            workers.retain(|worker| !worker.is_finished());
            if Instant::now() >= next_sample {
                next_sample = self.sample_pressure();
            }
            if let Some(telemetry) = &self.reads.telemetry
                && Instant::now() >= next_report
            {
                let lag = self.workload_projection_lag(telemetry);
                let projection_fault = self
                    .projection_error
                    .try_lock()
                    .ok()
                    .map(|fault| fault.is_some());
                telemetry.report(&mut self_report, lag, projection_fault);
                next_report = Instant::now() + REPORT_INTERVAL;
            }
            if projector.is_finished()
                && let Ok(mut fault) = self.projection_error.lock()
            {
                fault.get_or_insert_with(|| {
                    "analytical projection stopped; retained canonical records require recovery"
                        .to_string()
                });
            }
            match self.listener.accept() {
                Ok((socket, _)) => {
                    if !elastic_can_admit(
                        connection_limit as u64,
                        active.load(Ordering::SeqCst) as u64,
                    ) {
                        continue;
                    }
                    active.fetch_add(1, Ordering::SeqCst);
                    let guard = DropGuard::new(active.clone(), |active| {
                        active.fetch_sub(1, Ordering::SeqCst);
                    });
                    let ledger = self.ledger.clone();
                    let projection = self.projection.clone();
                    let auth = self.auth.clone();
                    let projection_error = self.projection_error.clone();
                    let reads = self.reads.clone();
                    let worker = thread::Builder::new().spawn(move || {
                        let _connection_lease = guard;
                        let mut socket = socket;
                        let mut query_lease = None;
                        let request = canonical::read_frame(
                            &mut socket,
                            Instant::now() + Duration::from_secs(2),
                        )
                        .and_then(|b| {
                            serde_json::from_slice::<Authenticated>(&b).map_err(|e| e.to_string())
                        });
                        let response = match request {
                            Ok(req) => handle(
                                req,
                                &ledger,
                                &projection,
                                &auth,
                                &projection_error,
                                &reads,
                                &mut query_lease,
                            ),
                            Err(e) => Err(e),
                        }
                        .unwrap_or_else(|error| Response::Error { error });
                        let body = serde_json::to_vec(&response).and_then(|b| {
                            if b.len() <= canonical::MAX_FRAME {
                                Ok(b)
                            } else {
                                serde_json::to_vec(&Response::Error {
                                    error: "response byte limit".into(),
                                })
                            }
                        });
                        if let Ok(body) = body {
                            let _ = canonical::write_frame(
                                &mut socket,
                                &body,
                                Instant::now() + Duration::from_secs(2),
                            );
                        }
                    });
                    match worker {
                        Ok(worker) => workers.push(worker),
                        Err(e) if retry_accept(&e) => thread::sleep(Duration::from_millis(5)),
                        Err(e) => {
                            failure = Some(e.to_string());
                            break;
                        }
                    }
                }
                Err(e) if retry_accept(&e) => thread::sleep(Duration::from_millis(5)),
                Err(e) => {
                    failure = Some(e.to_string());
                    break;
                }
            }
        }
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(q) = self.reads.queries.lock() {
            q.cancel.store(true, Ordering::SeqCst)
        }
        for w in workers {
            let _ = w.join();
        }
        if let Some(telemetry) = &self.reads.telemetry {
            telemetry.shutdown();
        }
        match projector.join() {
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err("projection worker panicked".into()),
            _ => (),
        }
        failure.map_or(Ok(()), Err)
    }

    fn sample_pressure(&self) -> Instant {
        let pressure = resources::descriptor_pressure() || resources::process_memory().pressured;
        self.reads.pressure.store(pressure, Ordering::Release);
        Instant::now() + Duration::from_millis(resources::RECEIVER_PRESSURE_SAMPLE_MS)
    }

    fn workload_projection_lag(&self, telemetry: &ServerTelemetry) -> Option<u64> {
        if self.reads.engine.is_some() {
            return None;
        }
        let progress = self.projection.try_lock().ok()?.progress();
        let canonical = self.reads.frontiers.try_read().ok()?;
        canonical
            .iter()
            .filter(|(origin, _)| telemetry.tracks(origin))
            .map(|(origin, through)| {
                through.saturating_sub(progress.frontiers.get(origin).copied().unwrap_or(0))
            })
            .max()
    }
}
fn retry_accept(error: &std::io::Error) -> bool {
    // Existing work can release transient pressure without abandoning custody.
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::ConnectionAborted
    ) || resources::is_resource_error(error)
}
fn retry_projector_pressure(error: &str) -> bool {
    use rustix::io::Errno;
    error == "unable to open database file"
        || [Errno::MFILE, Errno::NFILE, Errno::NOBUFS, Errno::NOMEM]
            .iter()
            .any(|errno| {
                let msg = std::io::Error::from_raw_os_error(errno.raw_os_error()).to_string();
                error.contains(&msg)
            })
}
// FIFO admission prevents a hot writer from repeatedly bypassing a waiting
// read. The inner standard mutex retains poisoning after a storage panic.
struct CanonicalGuard<'a> {
    state: std::sync::MutexGuard<'a, Ledger>,
    _admission: FairMutexGuard<'a, ()>,
}
impl std::ops::Deref for CanonicalGuard<'_> {
    type Target = Ledger;
    fn deref(&self) -> &Ledger {
        &self.state
    }
}
impl std::ops::DerefMut for CanonicalGuard<'_> {
    fn deref_mut(&mut self) -> &mut Ledger {
        &mut self.state
    }
}
fn canonical_state<'a>(
    ledger: &'a Arc<Mutex<Ledger>>,
    reads: &'a ReadState,
) -> Result<CanonicalGuard<'a>, String> {
    let admission = reads
        .admission
        .try_lock_for(Duration::from_millis(500))
        .ok_or("canonical storage admission deadline")?;
    let state = ledger.lock().map_err(|_| "canonical storage unavailable")?;
    Ok(CanonicalGuard {
        state,
        _admission: admission,
    })
}

fn optional_admission(pressure: bool, active: bool) -> bool {
    let owned = u64::from(active);
    let returned = elastic_lease_return(1, owned, 1, pressure);
    elastic_can_admit(1 - returned, owned)
}

fn reserve(
    reads: &ReadState,
    id: &str,
    lease: &mut Option<QueryGuard>,
) -> Result<Arc<AtomicBool>, String> {
    let mut slot = reads.queries.lock().map_err(|_| "query lock")?;
    if !optional_admission(reads.pressure.load(Ordering::Acquire), slot.active) {
        return Err(if slot.active {
            "query busy".into()
        } else {
            "analytical work deferred by resource pressure".into()
        });
    }
    let cancel = Arc::new(AtomicBool::new(false));
    slot.active = true;
    slot.id = id.to_owned();
    slot.cancel = cancel.clone();
    *lease = Some(QueryGuard(reads.queries.clone()));
    Ok(cancel)
}
fn receipt_readiness(
    request: &ProjectionReadyRequest,
    reads: &ReadState,
    projection: &Mutex<Projection>,
    projection_error: &Mutex<Option<String>>,
    query_lease: &mut Option<QueryGuard>,
) -> ProjectionReadiness {
    let deadline = Instant::now() + Duration::from_millis(request.timeout_ms.into());
    let mut report = ProjectionReadiness {
        status: ProjectionStatus::Unavailable,
        authority: None,
        reason: ProjectionReason::AuthorityMismatch,
        custody_frontier: None,
        projection_frontier: None,
    };
    if request.authority != reads.authority {
        return report;
    }
    report.authority = Some(reads.authority.clone());
    let Ok(frontiers) = reads.frontiers.try_read() else {
        report.reason = ProjectionReason::CustodyEvidenceUnavailable;
        return report;
    };
    let custody = frontiers.get(&request.origin).copied().unwrap_or(0);
    report.custody_frontier = Some(custody);
    drop(frontiers);
    if reads.engine.is_some() {
        // Native custody has no independently committed Parquet witness.
        report.reason = ProjectionReason::BackendUnsupported;
        return report;
    }
    if let Ok(current) = projection.try_lock() {
        report.projection_frontier = current.source_frontier(&request.origin).ok();
    }
    if request.source_id > custody {
        report.reason = ProjectionReason::RecordUnavailable;
        return report;
    }
    report.reason = ProjectionReason::ProjectionEvidenceUnavailable;
    let Ok(cancel) = reserve(reads, "projection-ready", query_lease) else {
        return report;
    };
    let row = {
        let Ok(reader) = reads.reader.try_lock() else {
            report.reason = ProjectionReason::CustodyEvidenceUnavailable;
            return report;
        };
        let Ok(rows) = reader.read(&request.origin, request.source_id - 1, request.source_id, 1)
        else {
            report.reason = ProjectionReason::CustodyEvidenceUnavailable;
            return report;
        };
        let Some(row) = rows.into_iter().next() else {
            report.reason = ProjectionReason::RecordUnavailable;
            return report;
        };
        row
    };
    let Ok(observation) = observation::decode_versioned(row.raw.as_bytes()) else {
        report.reason = ProjectionReason::CustodyEvidenceUnavailable;
        return report;
    };
    if row.origin != request.origin
        || row.source_id != request.source_id
        || observation.identity() != &request.identity
    {
        report.reason = ProjectionReason::IdentityMismatch;
        return report;
    }
    while Instant::now() < deadline {
        if cancel.load(Ordering::Acquire)
            || !matches!(projection_error.try_lock(), Ok(error) if error.is_none())
        {
            report.status = ProjectionStatus::Unavailable;
            report.reason = ProjectionReason::ProjectionEvidenceUnavailable;
            return report;
        }
        match projection.try_lock() {
            Ok(projected) => {
                let Ok(frontier) = projected.source_frontier(&request.origin) else {
                    report.status = ProjectionStatus::Unavailable;
                    report.reason = ProjectionReason::ProjectionEvidenceUnavailable;
                    return report;
                };
                report.projection_frontier = Some(frontier);
                if frontier >= request.source_id {
                    report.status = ProjectionStatus::Unavailable;
                    report.reason = ProjectionReason::ProjectionEvidenceUnavailable;
                    match projected.proves_projected_row(&row, &cancel, deadline) {
                        Ok(true) => {
                            report.status = ProjectionStatus::Visible;
                            report.reason = ProjectionReason::CommittedRow;
                        }
                        Ok(false) => report.reason = ProjectionReason::ProjectedRowUnavailable,
                        Err(_) => {}
                    }
                    return report;
                }
                report.status = ProjectionStatus::Pending;
                report.reason = ProjectionReason::AwaitingProjection;
            }
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Poisoned(_)) => {
                report.status = ProjectionStatus::Unavailable;
                report.reason = ProjectionReason::ProjectionEvidenceUnavailable;
                return report;
            }
        }
        if !request.wait {
            break;
        }
        thread::sleep(
            Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    report
}

fn handle(
    req: Authenticated,
    ledger: &Arc<Mutex<Ledger>>,
    projection: &Arc<Mutex<Projection>>,
    auth: &Auth,
    projection_error: &Mutex<Option<String>>,
    reads: &ReadState,
    query_lease: &mut Option<QueryGuard>,
) -> Result<Response, String> {
    let reader = matches(&auth.reader, &req.credential);
    let node = auth
        .nodes
        .iter()
        .find(|(_, key)| matches(key, &req.credential))
        .map(|(id, _)| id.clone());
    if !reader && node.is_none() {
        return Err("unauthorized".into());
    }
    let allows =
        |origin: &str| reader && auth.nodes.contains_key(origin) || node.as_deref() == Some(origin);
    let mut measurement = reads
        .telemetry
        .as_ref()
        .and_then(|telemetry| telemetry.measure(&req.request, node.as_deref(), reader, auth));
    match req.request {
        Request::Ingest { origin, records } => {
            if node.as_deref() != Some(&origin) {
                return Err("unauthorized origin".into());
            }
            let mut ledger = canonical_state(ledger, reads)?;
            let result = ledger.ingest(&origin, &records);
            *reads
                .frontiers
                .write()
                .map_err(|_| "canonical catalog lock")? = ledger.frontiers()?;
            let through = result?;
            if let Some(observation) = &mut measurement {
                observation.succeed();
            }
            Ok(Response::Committed {
                authority: ledger.authority.clone(),
                origin,
                through,
            })
        }
        Request::Read {
            origin,
            after,
            through,
            limit,
        } => {
            if !allows(&origin) {
                return Err("unauthorized origin".into());
            }
            Ok(Response::Rows {
                rows: if reads.engine.is_some() {
                    reads
                        .reader
                        .lock()
                        .map_err(|_| "canonical read lock")?
                        .read(&origin, after, through, limit)?
                } else {
                    canonical_state(ledger, reads)?.read(&origin, after, through, limit)?
                },
            })
        }
        Request::Lookup { origin, identity } => {
            if !allows(&origin) {
                return Err("unauthorized origin".into());
            }
            Ok(Response::Found {
                row: if reads.engine.is_some() {
                    reads
                        .reader
                        .lock()
                        .map_err(|_| "canonical read lock")?
                        .lookup(&origin, &identity)?
                } else {
                    canonical_state(ledger, reads)?.lookup(&origin, &identity)?
                },
            })
        }
        Request::Progress => {
            let mut frontiers = reads
                .frontiers
                .read()
                .map_err(|_| "canonical catalog lock")?
                .clone();
            frontiers.retain(|origin, _| allows(origin));
            Ok(Response::Progress {
                authority: reads.authority.clone(),
                frontiers,
            })
        }
        Request::ProjectionReady { request } => {
            if !reader {
                return Err("read credential required".into());
            }
            request.validate()?;
            let readiness = if allows(&request.origin) {
                receipt_readiness(&request, reads, projection, projection_error, query_lease)
            } else {
                ProjectionReadiness {
                    status: ProjectionStatus::Unavailable,
                    authority: None,
                    reason: ProjectionReason::UnknownOrigin,
                    custody_frontier: None,
                    projection_frontier: None,
                }
            };
            Ok(Response::ProjectionReady { readiness })
        }
        Request::Capabilities => Ok(Response::Capabilities {
            authority: reads.authority.clone(),
            schemas: vec![2, 3],
            origins: auth.nodes.keys().filter(|o| allows(o)).cloned().collect(),
            max_frame: canonical::MAX_FRAME,
            max_batch_records: canonical::MAX_BATCH,
        }),
        Request::Evidence { canonical_id } => {
            if !reader {
                return Err("read credential required".into());
            }
            let row = if reads.engine.is_some() {
                reads
                    .reader
                    .lock()
                    .map_err(|_| "canonical read lock")?
                    .evidence(canonical_id)?
            } else {
                canonical_state(ledger, reads)?.evidence(canonical_id)?
            }
            .filter(|r| allows(&r.origin));
            Ok(Response::Found { row })
        }
        Request::Cancel { query_id } => {
            if !reader {
                return Err("read credential required".into());
            }
            let slot = reads.queries.lock().map_err(|_| "query lock")?;
            if !slot.active || slot.id != query_id {
                return Err("query unavailable".into());
            }
            slot.cancel.store(true, Ordering::SeqCst);
            Ok(Response::Cancelled { query_id })
        }
        Request::Plan { mut request } => {
            if !reader {
                return Err("read credential required".into());
            }
            let canonical = prepare(&mut request, auth, reads)?;
            let cancel = reserve(reads, &request.query_id, query_lease)?;
            let deadline =
                Instant::now() + Duration::from_millis(request.budgets.timeout_ms.into());
            let plan = if let Some(engine) = &reads.engine {
                engine.plan(request, canonical, &cancel, deadline)?
            } else {
                if let Some(e) = &*projection_error
                    .lock()
                    .map_err(|_| "projection fault lock")?
                {
                    return Err(format!("analytical projection unavailable: {e}"));
                }
                projection
                    .try_lock()
                    .map_err(|_| "projection busy")?
                    .plan(request)?
            };
            Ok(Response::Plan { plan })
        }
        Request::Query { mut request } => {
            if !reader {
                return Err("read credential required".into());
            }
            let canonical = prepare(&mut request, auth, reads)?;
            let output_limit = request.budgets.output_bytes as usize;
            let cancel = reserve(reads, &request.query_id, query_lease)?;
            let deadline =
                Instant::now() + Duration::from_millis(request.budgets.timeout_ms.into());
            let authority = request
                .authority
                .clone()
                .ok_or("unbound canonical authority")?;
            let mut response = if let Some(engine) = &reads.engine {
                engine.query(request, canonical.clone(), &cancel, deadline)?
            } else {
                loop {
                    if cancel.load(Ordering::SeqCst) {
                        return Err("query cancelled".into());
                    }
                    if Instant::now() >= deadline {
                        return Err(
                            "projection deadline; requested canonical fence unavailable".into()
                        );
                    }
                    if let Some(e) = &*projection_error
                        .lock()
                        .map_err(|_| "projection fault lock")?
                    {
                        return Err(format!("analytical projection unavailable: {e}"));
                    }
                    let p = projection.lock().map_err(|_| "projection lock")?;
                    let progress = p.progress();
                    if request
                        .snapshot
                        .iter()
                        .all(|(o, n)| progress.frontiers.get(o).copied().unwrap_or(0) >= *n)
                    {
                        break;
                    }
                    drop(p);
                    thread::sleep(Duration::from_millis(10));
                }
                projection
                    .lock()
                    .map_err(|_| "projection lock")?
                    .query_with_cancel_deadline(request, &cancel, deadline)?
            };
            response.coverage.canonical_frontier = canonical;
            response.coverage.canonical_authority = authority;
            for _ in 0..4 {
                let measured = serde_json::to_vec(&Response::Query {
                    response: response.clone(),
                })
                .map_err(|e| e.to_string())?
                .len();
                response.coverage.consumed.output_bytes = measured as u32;
            }
            let measured = serde_json::to_vec(&Response::Query {
                response: response.clone(),
            })
            .map_err(|e| e.to_string())?
            .len();
            if measured > output_limit || measured > canonical::MAX_FRAME {
                return Err("query response envelope byte budget".into());
            }
            if let Some(observation) = &mut measurement {
                observation.succeed();
            }
            Ok(Response::Query { response })
        }
    }
}
fn prepare(
    request: &mut QueryRequest,
    auth: &Auth,
    reads: &ReadState,
) -> Result<BTreeMap<String, u64>, String> {
    if request.scope.is_empty() {
        request.scope = auth.nodes.keys().cloned().collect()
    }
    request.validate()?;
    if request.scope.iter().any(|o| !auth.nodes.contains_key(o)) {
        return Err("unauthorized scope".into());
    }
    let mut canonical = reads
        .frontiers
        .read()
        .map_err(|_| "canonical catalog lock")?
        .clone();
    let authority = reads.authority.clone();
    if request.authority.as_ref().is_some_and(|a| a != &authority) {
        return Err("canonical authority mismatch".into());
    }
    request.authority = Some(authority);
    canonical.retain(|o, _| request.scope.contains(o));
    for origin in &request.scope {
        canonical.entry(origin.clone()).or_insert(0);
    }
    if request.snapshot.is_empty() && request.cursor.is_none() {
        request.snapshot = canonical.clone()
    }
    if request
        .snapshot
        .iter()
        .any(|(o, n)| !request.scope.contains(o) || *n > canonical.get(o).copied().unwrap_or(0))
    {
        return Err("requested canonical fence unavailable".into());
    }
    Ok(canonical)
}
pub fn cli() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["serve", config] => {
            let server = Server::open(fabric::config::load(Path::new(config))?)?;
            let stop = server.stop_handle();
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
            thread::Builder::new()
                .name("fabric-server-signal".into())
                .spawn(move || {
                    let rt = match tokio::runtime::Builder::new_current_thread()
                        .enable_io()
                        .build()
                    {
                        Ok(rt) => rt,
                        Err(e) => {
                            let _ = ready_tx.send(Err(format!("signal runtime error: {e}")));
                            return;
                        }
                    };
                    rt.block_on(async move {
                        let mut sigterm = match tokio::signal::unix::signal(
                            tokio::signal::unix::SignalKind::terminate(),
                        ) {
                            Ok(s) => s,
                            Err(e) => {
                                let _ = ready_tx.send(Err(format!("sigterm handler error: {e}")));
                                return;
                            }
                        };
                        let mut sigint = match tokio::signal::unix::signal(
                            tokio::signal::unix::SignalKind::interrupt(),
                        ) {
                            Ok(s) => s,
                            Err(e) => {
                                let _ = ready_tx.send(Err(format!("sigint handler error: {e}")));
                                return;
                            }
                        };
                        let _ = ready_tx.send(Ok(()));
                        tokio::select! {
                            _ = sigterm.recv() => {}
                            _ = sigint.recv() => {}
                        }
                        stop.store(true, Ordering::SeqCst);
                    });
                })
                .map_err(|e| format!("signal thread spawn error: {e}"))?;
            ready_rx
                .recv()
                .map_err(|_| "signal thread exited before registering handlers".to_string())??;
            let resources = resources::receiver_resources(server.max_connections);
            let connection_limit = resources.connection_limit;
            println!(
                "{}",
                serde_json::json!({"ready": true, "address": server.address(), "connection_limit": connection_limit, "resources": resources})
            );
            std::io::stdout().flush().map_err(|e| e.to_string())?;
            fabric::spindle::notify_systemd_ready();
            server.serve_with_limit(connection_limit)
        }
        _ => Err("usage: fabric-server serve CONFIG".into()),
    }
}

#[cfg(test)]
mod receipt_tests {
    use super::*;
    use fabric::canonical::ForwardRecord;

    fn fixture() -> (tempfile::TempDir, Server) {
        let scratch = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/var/tmp"));
        assert!(!scratch.starts_with("/tmp") && !scratch.starts_with("/dev/shm"));
        let root = tempfile::tempdir_in(scratch).unwrap();
        let credential = |name: &str| {
            let path = root.path().join(name);
            fs::write(&path, format!("canonical-test-credential-{name}")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            path
        };
        let server = Server::open(Config {
            listen: "127.0.0.1:0".into(),
            storage: root.path().join("canonical"),
            projection: root.path().join("projection"),
            quota_bytes: 4 << 20,
            projection_quota_bytes: 4 << 20,
            reader_credential_file: credential("reader"),
            nodes: ["alpha", "beta"]
                .into_iter()
                .map(|origin| NodeGrant {
                    origin: origin.into(),
                    credential_file: credential(origin),
                })
                .collect(),
            node_quota_bytes: Default::default(),
            engine_endpoint: None,
            max_connections: 4,
        })
        .unwrap();
        (root, server)
    }

    fn call(server: &Server, credential: &str, request: Request) -> Result<Response, String> {
        handle(
            Authenticated {
                credential: format!("canonical-test-credential-{credential}"),
                request,
            },
            &server.ledger,
            &server.projection,
            &server.auth,
            &server.projection_error,
            &server.reads,
            &mut None,
        )
    }

    fn probe(server: &Server, request: &ProjectionReadyRequest) -> ProjectionReadiness {
        match call(
            server,
            "reader",
            Request::ProjectionReady {
                request: request.clone(),
            },
        )
        .unwrap()
        {
            Response::ProjectionReady { readiness } => readiness,
            other => panic!("unexpected probe response {other:?}"),
        }
    }

    #[test]
    fn pressure_keeps_live_query_lease_until_cleanup() {
        let (_root, server) = fixture();
        let mut lease = None;
        let cancel = reserve(&server.reads, "one", &mut lease).unwrap();
        server.reads.pressure.store(true, Ordering::Release);
        assert!(matches!(
            call(
                &server,
                "reader",
                Request::Cancel {
                    query_id: "one".into()
                }
            ),
            Ok(Response::Cancelled { .. })
        ));
        assert!(cancel.load(Ordering::Acquire));
        assert_eq!(
            reserve(&server.reads, "two", &mut None).unwrap_err(),
            "query busy"
        );
        drop(lease.take());
        assert_eq!(
            reserve(&server.reads, "two", &mut None).unwrap_err(),
            "analytical work deferred by resource pressure"
        );
        server.reads.pressure.store(false, Ordering::Release);
        assert!(reserve(&server.reads, "two", &mut lease).is_ok());
    }

    #[test]
    fn exact_receipt_requires_a_queryable_committed_row() {
        let (root, server) = fixture();
        let raw = |name: &str| {
            serde_json::json!({
                "version": 2, "registry": "fabrico11y/2",
                "identity": {"producer": name, "generation": "sample", "sequence": 1},
                "timestamp_ns": 0, "resource": "service",
                "data": {"kind": "log", "level": "info", "message": "sample"}
            })
            .to_string()
        };
        for origin in ["alpha", "beta"] {
            assert!(matches!(
                call(
                    &server,
                    origin,
                    Request::Ingest {
                        origin: origin.into(),
                        records: vec![ForwardRecord {
                            source_id: 1,
                            raw: raw(origin),
                        }],
                    },
                ),
                Ok(Response::Committed { through: 1, .. })
            ));
        }
        let request = ProjectionReadyRequest {
            authority: server.reads.authority.clone(),
            origin: "beta".into(),
            source_id: 1,
            identity: observation::decode_versioned(raw("beta").as_bytes())
                .unwrap()
                .identity()
                .clone(),
            timeout_ms: 1000,
            wait: false,
        };
        assert!(matches!(
            probe(&server, &request),
            ProjectionReadiness {
                status: ProjectionStatus::Pending,
                custody_frontier: Some(1),
                projection_frontier: Some(0),
                ..
            }
        ));
        let mut wait = request.clone();
        wait.timeout_ms = 50;
        wait.wait = true;
        let start = Instant::now();
        assert_eq!(probe(&server, &wait).status, ProjectionStatus::Pending);
        assert!(start.elapsed() >= Duration::from_millis(50));
        assert!(start.elapsed() < Duration::from_secs(1));

        let rows = server.ledger.lock().unwrap().replay(0, 2, 8192).unwrap();
        server.projection.lock().unwrap().apply(&rows[..1]).unwrap();
        assert_eq!(probe(&server, &request).status, ProjectionStatus::Pending);
        server.projection.lock().unwrap().apply(&rows[1..]).unwrap();
        assert!(matches!(
            probe(&server, &request),
            ProjectionReadiness {
                status: ProjectionStatus::Visible,
                reason: ProjectionReason::CommittedRow,
                custody_frontier: Some(1),
                projection_frontier: Some(1),
                ..
            }
        ));

        let mut mismatch = request.clone();
        mismatch.source_id = 2;
        assert_eq!(
            probe(&server, &mismatch).reason,
            ProjectionReason::RecordUnavailable,
        );
        mismatch.source_id = 1;
        mismatch.identity.producer = "alpha".into();
        assert_eq!(
            probe(&server, &mismatch).reason,
            ProjectionReason::IdentityMismatch,
        );
        mismatch.authority = "wrong-authority".into();
        assert!(matches!(
            probe(&server, &mismatch),
            ProjectionReadiness {
                reason: ProjectionReason::AuthorityMismatch,
                custody_frontier: None,
                projection_frontier: None,
                ..
            }
        ));
        mismatch.authority = request.authority.clone();
        mismatch.origin = "not-granted".into();
        assert_eq!(
            probe(&server, &mismatch).reason,
            ProjectionReason::UnknownOrigin,
        );
        assert!(
            call(
                &server,
                "beta",
                Request::ProjectionReady {
                    request: request.clone(),
                },
            )
            .is_err()
        );

        let segment = root.path().join("projection/segment-2-0.parquet");
        assert!(segment.is_file());
        let mut bytes = fs::read(&segment).unwrap();
        bytes[16] ^= 1;
        fs::write(segment, bytes).unwrap();
        assert!(matches!(
            probe(&server, &request),
            ProjectionReadiness {
                status: ProjectionStatus::Unavailable,
                reason: ProjectionReason::ProjectionEvidenceUnavailable,
                custody_frontier: Some(1),
                projection_frontier: Some(1),
                ..
            }
        ));
    }
}

#[cfg(test)]
mod config_example_test {
    #[test]
    fn receiver_example_uses_executable_schema() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/server.toml");
        let config: super::Config = fabric::config::load(&path).unwrap();
        assert!(
            config
                .listen
                .parse::<std::net::SocketAddr>()
                .unwrap()
                .ip()
                .is_loopback()
        );
        assert_eq!(config.nodes.len(), 1);
    }
}
