//! Bounded authoritative receiver and isolated analytical projection worker.
use admission_kernel::telemetry::{elastic_can_admit, elastic_lease_return};

#[cfg(test)]
mod acceptance_tests;
pub mod alert_journal;
pub mod engine;
mod handshake;
mod handshake_io;
pub mod ledger;
mod plan_cursor;
mod segment_ticket;
mod self_observation;
mod worker_pool;
use fabric::{
    canonical::{
        self, ProjectionReadiness, ProjectionReadyRequest, ProjectionReason, ProjectionStatus,
        Request, Response,
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
/// One operator-provisioned, immutable plan scope owner.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionWriterGrant {
    pub writer: String,
    pub scope: String,
    pub origin: String,
    pub producer: String,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decision_writers: Vec<DecisionWriterGrant>,
    /// Optional retained payload limits for dedicated telemetry origins.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub node_quota_bytes: BTreeMap<String, u64>,
    #[serde(default)]
    pub engine_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_auth_token_file: Option<PathBuf>,
    #[serde(default)]
    pub allow_unauthenticated_loopback_engine: bool,
    #[serde(default = "connections")]
    pub max_connections: usize,
}
fn connections() -> usize {
    resources::DEFAULT_RECEIVER_CONNECTIONS
}
fn secret(path: &Path) -> Result<String, String> {
    use rustix::fs::{Mode, OFlags};
    use std::io::Read;
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| format!("failed opening credential {}: {e}", path.display()))?;
    let mut file = fs::File::from(fd);
    let m = file.metadata().map_err(|e| e.to_string())?;
    if !m.is_file()
        || m.uid() != rustix::process::getuid().as_raw()
        || m.permissions().mode() & 0o077 != 0
        || m.len() > 256
    {
        return Err("credential must be a private owned regular file <=256 bytes".into());
    }
    let mut s = String::new();
    (&mut file)
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
    writers: BTreeMap<String, String>,
    writer_bindings: BTreeMap<String, handshake::WriterBinding>,
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
    ticket_key: [u8; 32],
    ticket_epoch: Instant,
    frontiers: RwLock<BTreeMap<String, u64>>,
    engine: Option<engine::Greptime>,
    telemetry: Option<ServerTelemetry>,
    plan_hints: Mutex<plan_cursor::Hints>,
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
            || !(1..=resources::MAX_RECEIVER_CONNECTIONS).contains(&config.max_connections)
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
        let mut writers = BTreeMap::new();
        let mut writer_bindings = BTreeMap::new();
        let mut scopes = std::collections::BTreeSet::new();
        let valid = |s: &str| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
        };
        if config.decision_writers.len() > 16 {
            return Err("too many decision writers".into());
        }
        for grant in &config.decision_writers {
            if !valid(&grant.writer)
                || !valid(&grant.scope)
                || !valid(&grant.producer)
                || !nodes.contains_key(&grant.origin)
                || writers.contains_key(&grant.writer)
                || !scopes.insert(grant.scope.clone())
            {
                return Err("invalid decision writer grant".into());
            }
            let token = secret(&grant.credential_file)?;
            if matches(&reader, &token)
                || nodes
                    .values()
                    .chain(writers.values())
                    .any(|s| matches(s, &token))
            {
                return Err("credentials must have disjoint grants".into());
            }
            writers.insert(grant.writer.clone(), token);
            writer_bindings.insert(
                grant.writer.clone(),
                handshake::WriterBinding {
                    writer: grant.writer.clone(),
                    scope: grant.scope.clone(),
                    origin: grant.origin.clone(),
                    producer: grant.producer.clone(),
                },
            );
        }
        if config
            .node_quota_bytes
            .keys()
            .any(|origin| !nodes.contains_key(origin))
        {
            return Err("canonical node quota requires a configured origin".into());
        }
        let engine_auth_token = match (&config.engine_endpoint, &config.engine_auth_token_file) {
            (Some(_), Some(token_path)) => Some(secret(token_path)?),
            (Some(_), None) if config.allow_unauthenticated_loopback_engine => None,
            (Some(_), None) => {
                return Err(
                    "engine_endpoint requires engine_auth_token_file unless allow_unauthenticated_loopback_engine is true"
                        .into(),
                );
            }
            (None, Some(_)) => {
                return Err("engine_auth_token_file requires engine_endpoint".into());
            }
            (None, None) => None,
        };
        let enrolled_origins: Vec<String> = nodes.keys().cloned().collect();
        let omitted_count = enrolled_origins
            .iter()
            .filter(|origin| !config.node_quota_bytes.contains_key(*origin))
            .count();
        let explicit_canonical_sum: u64 = config.node_quota_bytes.values().copied().sum();
        let usable_canonical_pool = if config.engine_endpoint.is_some() {
            config
                .quota_bytes
                .saturating_sub(ledger::RESERVE)
                .saturating_sub(explicit_canonical_sum)
        } else {
            (config.quota_bytes.saturating_sub(ledger::RESERVE) / 2)
                .saturating_sub(explicit_canonical_sum)
        };
        let default_canonical_origin_quota = if omitted_count > 0 {
            Some((usable_canonical_pool / omitted_count as u64).max(1))
        } else {
            None
        };
        let ledger = Arc::new(Mutex::new(Ledger::open_with_quotas_and_auth(
            &config.storage,
            config.quota_bytes,
            config.engine_endpoint.as_deref(),
            config.node_quota_bytes,
            default_canonical_origin_quota,
            enrolled_origins.clone(),
            engine_auth_token.as_deref(),
            config.allow_unauthenticated_loopback_engine,
        )?));
        ledger
            .lock()
            .map_err(|_| "canonical lock")?
            .provision_plan_scopes(&config.decision_writers)?;
        ledger::private_dir(&config.projection)?;
        let projection_usable = config.projection_quota_bytes.saturating_sub(64 * 1024);
        let default_projection_origin_quota =
            Some((projection_usable / enrolled_origins.len() as u64).max(1));
        let mut projection_state = Projection::open_with_origin_quotas(
            &config.projection,
            config.projection_quota_bytes,
            BTreeMap::new(),
            default_projection_origin_quota,
        )?;
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
        let auth = Auth {
            reader,
            nodes,
            writers,
            writer_bindings,
        };
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
                ticket_key: segment_ticket::new_key()?,
                ticket_epoch: Instant::now(),
                frontiers: RwLock::new(frontiers),
                engine: state.engine(),
                telemetry,
                plan_hints: Mutex::new(plan_cursor::Hints::default()),
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
        let pool = Arc::new(worker_pool::ConnectionWorkerPool::new(
            connection_limit.clamp(2, 16),
            connection_limit.max(4),
        )?);
        let handshake_auth = Arc::new(handshake::HandshakeAuth {
            reader: self.auth.reader.clone(),
            nodes: self.auth.nodes.clone(),
            writers: self.auth.writers.clone(),
            writer_bindings: self.auth.writer_bindings.clone(),
            authority: self.reads.authority.clone(),
        });
        let preauth_limit = (connection_limit * 4).clamp(16, 128);
        let preauth =
            handshake::PreAuthGate::spawn(self.stop.clone(), handshake_auth, preauth_limit, {
                let active = Arc::clone(&active);
                let pool = Arc::clone(&pool);
                let ledger = Arc::clone(&self.ledger);
                let projection = Arc::clone(&self.projection);
                let auth = Arc::clone(&self.auth);
                let projection_error = Arc::clone(&self.projection_error);
                let reads = Arc::clone(&self.reads);
                move |session| {
                    if !elastic_can_admit(
                        connection_limit as u64,
                        active.load(Ordering::SeqCst) as u64,
                    ) {
                        return;
                    }
                    active.fetch_add(1, Ordering::SeqCst);
                    let guard = DropGuard::new(Arc::clone(&active), |active| {
                        active.fetch_sub(1, Ordering::SeqCst);
                    });
                    let ledger = Arc::clone(&ledger);
                    let projection = Arc::clone(&projection);
                    let auth = Arc::clone(&auth);
                    let projection_error = Arc::clone(&projection_error);
                    let reads = Arc::clone(&reads);
                    let _ = pool.try_submit(move || {
                        let _connection_lease = guard;
                        let handshake::AuthenticatedSession {
                            stream: mut socket,
                            peer,
                            client_nonce,
                            server_nonce,
                            request_mac,
                        } = session;
                        let request = canonical::read_frame(
                            &mut socket,
                            Instant::now() + Duration::from_secs(2),
                        )
                        .and_then(|bytes| {
                            let grant = peer.verify_request(
                                &client_nonce,
                                &server_nonce,
                                &bytes,
                                &request_mac,
                            )?;
                            observation::check_json_depth(
                                &bytes,
                                canonical::MAX_FRAME,
                                observation::MAX_DEPTH + 6,
                            )?;
                            let req: Request =
                                serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                            Ok((grant, req))
                        });
                        respond(
                            request,
                            &ledger,
                            &projection,
                            &auth,
                            &projection_error,
                            &reads,
                            |body| {
                                let _ = canonical::write_frame(
                                    &mut socket,
                                    body,
                                    Instant::now() + Duration::from_secs(2),
                                );
                            },
                        );
                    });
                }
            })?;
        let mut failure = None;
        while !self.stop.load(Ordering::SeqCst) {
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
                    if !preauth.try_admit(socket) {
                        thread::sleep(Duration::from_millis(2));
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
        preauth.shutdown();
        if let Ok(pool) = Arc::try_unwrap(pool) {
            pool.shutdown();
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

fn receipt_batch_readiness(
    request: &canonical::ReceiptBatchRequest,
    reads: &ReadState,
    projection: &Mutex<Projection>,
    projection_error: &Mutex<Option<String>>,
    query_lease: &mut Option<QueryGuard>,
) -> canonical::ReceiptBatchReadiness {
    use canonical::{
        ReceiptBatchOutcome as Outcome, ReceiptBatchReadiness as Report,
        ReceiptBatchStatus as Status,
    };
    let deadline = Instant::now() + Duration::from_millis(request.timeout_ms.into());
    let mut report = Report {
        authority: request.authority.clone(),
        origin: request.origin.clone(),
        source_barrier: request.source_barrier,
        custody_frontier: None,
        projection_frontier: None,
        outcomes: request
            .items
            .iter()
            .cloned()
            .map(|item| Outcome {
                item,
                status: Status::Unavailable,
                reason: ProjectionReason::CustodyEvidenceUnavailable,
                canonical_sha256: None,
            })
            .collect(),
        segment_attempts: 0,
        segment_bytes_reserved: 0,
        segment_rows_reserved: 0,
    };
    if request.authority != reads.authority {
        for outcome in &mut report.outcomes {
            outcome.reason = ProjectionReason::AuthorityMismatch;
        }
        return report;
    }
    let Ok(frontiers) = reads.frontiers.try_read() else {
        return report;
    };
    let custody = frontiers.get(&request.origin).copied().unwrap_or(0);
    report.custody_frontier = Some(custody);
    drop(frontiers);
    if reads.engine.is_some() {
        for outcome in &mut report.outcomes {
            outcome.reason = ProjectionReason::BackendUnsupported;
        }
        return report;
    }
    for outcome in &mut report.outcomes {
        if outcome.item.source_id > request.source_barrier {
            outcome.reason = ProjectionReason::RecordUnavailable;
        } else if outcome.item.source_id > custody {
            outcome.status = Status::Missing;
            outcome.reason = ProjectionReason::RecordUnavailable;
        }
    }
    let Ok(cancel) = reserve(reads, "receipt-batch-ready", query_lease) else {
        return report;
    };
    let ids: Vec<u64> = request
        .items
        .iter()
        .filter(|item| item.source_id <= request.source_barrier && item.source_id <= custody)
        .map(|item| item.source_id)
        .collect();
    let rows = match reads.reader.try_lock() {
        Ok(reader) => reader.batch_read(&request.origin, &ids, deadline),
        Err(_) => return report,
    };
    let Ok(rows) = rows else {
        return report;
    };
    let mut verified = Vec::new();
    for (index, outcome) in report.outcomes.iter_mut().enumerate() {
        let id = outcome.item.source_id;
        if id > request.source_barrier || id > custody {
            continue;
        }
        let Some(row) = rows.get(&id) else {
            continue;
        };
        let row = match row {
            Ok(Some(row)) => row,
            Ok(None) => {
                outcome.status = Status::Missing;
                outcome.reason = ProjectionReason::RecordUnavailable;
                continue;
            }
            Err(()) => continue,
        };
        let Ok(observation) = observation::decode_versioned(row.raw.as_bytes()) else {
            continue;
        };
        if row.origin != request.origin
            || row.source_id != id
            || observation.identity() != &outcome.item.identity
        {
            outcome.status = Status::Mismatch;
            outcome.reason = ProjectionReason::IdentityMismatch;
            continue;
        }
        outcome.reason = ProjectionReason::ProjectionEvidenceUnavailable;
        verified.push((index, row.clone()));
    }
    let Ok(projected) = projection.try_lock() else {
        return report;
    };
    let Ok(frontier) = projected.source_frontier(&request.origin) else {
        return report;
    };
    report.projection_frontier = Some(frontier);
    let mut candidates = Vec::new();
    let mut indexes = Vec::new();
    for (index, row) in verified {
        let outcome = &mut report.outcomes[index];
        if !matches!(projection_error.try_lock(),Ok(error) if error.is_none())
            || Instant::now() >= deadline
        {
            continue;
        }
        if outcome.item.source_id > frontier {
            outcome.status = Status::Pending;
            outcome.reason = ProjectionReason::AwaitingProjection;
            continue;
        }
        indexes.push(index);
        candidates.push(row);
    }
    let (results, segments, bytes, rows_scanned) =
        projected.proves_projected_rows(&candidates, &cancel, deadline);
    report.segment_attempts = segments;
    report.segment_bytes_reserved = bytes;
    report.segment_rows_reserved = rows_scanned;
    for ((index, row), result) in indexes.into_iter().zip(candidates).zip(results) {
        let outcome = &mut report.outcomes[index];
        match result {
            Ok(true) => {
                outcome.status = Status::Ready;
                outcome.reason = ProjectionReason::CommittedRow;
                if request.include_digest {
                    outcome.canonical_sha256 = Some(
                        Sha256::digest(row.raw.as_bytes())
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect(),
                    );
                }
            }
            Ok(false) => {
                outcome.reason = ProjectionReason::ProjectedRowUnavailable;
            }
            Err(_) => {
                outcome.reason = ProjectionReason::ProjectionEvidenceUnavailable;
            }
        }
    }
    report
}

fn respond(
    request: Result<(handshake::AuthenticatedGrant, Request), String>,
    ledger: &Arc<Mutex<Ledger>>,
    projection: &Arc<Mutex<Projection>>,
    auth: &Auth,
    projection_error: &Mutex<Option<String>>,
    reads: &ReadState,
    write: impl FnOnce(&[u8]),
) {
    // The query lease covers execution, encoding, the write attempt and response cleanup.
    let mut query_lease = None;
    let response = request
        .and_then(|(grant, req)| {
            handle(
                &grant,
                req,
                ledger,
                projection,
                auth,
                projection_error,
                reads,
                &mut query_lease,
            )
        })
        .unwrap_or_else(|error| Response::Error { error });
    let body = serde_json::to_vec(&response).and_then(|body| {
        if body.len() <= canonical::MAX_FRAME {
            Ok(body)
        } else {
            serde_json::to_vec(&Response::Error {
                error: "response byte limit".into(),
            })
        }
    });
    if let Ok(body) = body {
        write(&body);
    }
}

#[allow(clippy::too_many_arguments)]
fn handle(
    grant: &handshake::AuthenticatedGrant,
    request: Request,
    ledger: &Arc<Mutex<Ledger>>,
    projection: &Arc<Mutex<Projection>>,
    auth: &Auth,
    projection_error: &Mutex<Option<String>>,
    reads: &ReadState,
    query_lease: &mut Option<QueryGuard>,
) -> Result<Response, String> {
    if grant.authority() != reads.authority {
        return Err("unauthorized".into());
    }
    let reader = grant.is_reader();
    let node = grant.node();
    let writer = grant.writer();
    if writer.is_some()
        && !matches!(
            &request,
            Request::AcceptPlan { .. }
                | Request::PlanPage { .. }
                | Request::RevokePlanCursors { .. }
                | Request::SubscribePlan { .. }
                | Request::PollPlanHint { .. }
        )
    {
        return Err("decision writer cannot read or ingest".into());
    }
    // Normalize the additive command only after authentication and reader
    // authorization, and before telemetry, preparation or analytical admission.
    let request = match request {
        Request::QueryWithOperationVersion {
            request,
            operation_version,
        } => {
            if !reader {
                return Err("read credential required".into());
            }
            let request = fabric::query::contracts::negotiate_v1(request, operation_version)
                .map_err(|_| "unsupported query operation contract")?
                .into_request();
            Request::Query { request }
        }
        request => request,
    };
    let allows = |origin: &str| reader && auth.nodes.contains_key(origin) || node == Some(origin);

    let mut measurement = reads
        .telemetry
        .as_ref()
        .and_then(|telemetry| telemetry.measure(&request, node, reader, auth));
    match request {
        Request::AcceptPlan { body } => {
            let writer = writer.ok_or("decision writer credential required")?;
            observation::check_json_depth(
                body.as_bytes(),
                canonical::MAX_ACCEPT_BODY,
                observation::MAX_DEPTH + 2,
            )?;
            let command: canonical::AcceptPlan =
                serde_json::from_str(&body).map_err(|_| "invalid acceptance command")?;
            if command.scope.0 != writer.scope || command.origin != writer.origin {
                return Err("unauthorized plan scope".into());
            }
            let mut state = canonical_state(ledger, reads)?;
            let result = state.accept_plan(writer, &body)?;
            if let Ok(mut hints) = reads.plan_hints.lock() {
                hints.signal(&command.scope.0);
            }
            drop(state); // Hold canonical admission through the advisory signal.
            Ok(Response::PlanAccepted { decision: result })
        }
        Request::PlanPage { request } => {
            let writer = writer.ok_or("decision writer credential required")?;
            match canonical_state(ledger, reads)?.plan_page(writer, &request)? {
                Ok(page) => Ok(Response::PlanPage { page }),
                Err(reason) => Ok(Response::PlanCursorFailure { reason }),
            }
        }
        Request::RevokePlanCursors { scope } => {
            let writer = writer.ok_or("decision writer credential required")?;
            let mut state = canonical_state(ledger, reads)?;
            state.revoke_plan_cursors(writer, &scope.0)?;
            if let Ok(mut hints) = reads.plan_hints.lock() {
                hints.revoke(&scope.0);
            }
            Ok(Response::PlanCursorsRevoked)
        }
        Request::SubscribePlan { scope } => {
            let writer = writer.ok_or("decision writer credential required")?;
            let state = canonical_state(ledger, reads)?;
            let (epoch, frontier) = state.plan_cursor_identity(writer, &scope.0)?;
            let mut hints = reads.plan_hints.lock().map_err(|_| "hint lock")?;
            let subscription = hints.subscribe(&writer.writer, &scope.0, epoch)?;
            drop(hints);
            // Registration and the captured frontier share the canonical lock with commit/signal.
            drop(state);
            match subscription {
                Some(subscription) => Ok(Response::PlanSubscribed {
                    subscription,
                    frontier,
                }),
                None => Ok(Response::PlanCursorFailure {
                    reason: canonical::PlanCursorFailure::Capacity,
                }),
            }
        }
        Request::PollPlanHint {
            scope,
            subscription,
        } => {
            let writer = writer.ok_or("decision writer credential required")?;
            let state = canonical_state(ledger, reads)?;
            let (epoch, _) = state.plan_cursor_identity(writer, &scope.0)?;
            let mut hints = reads.plan_hints.lock().map_err(|_| "hint lock")?;
            match hints.poll(&writer.writer, &scope.0, epoch, &subscription) {
                Ok(pending) => Ok(Response::PlanHint { pending }),
                Err(reason) => Ok(Response::PlanCursorFailure { reason }),
            }
        }
        Request::Ingest { origin, records } => {
            if node != Some(origin.as_str()) {
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
        Request::ExactReceiptLookup { request } => {
            if !reader {
                return Err("read credential required".into());
            }
            request.validate()?;
            if request.authority != reads.authority || !allows(&request.origin) {
                return Err("canonical authority or origin mismatch".into());
            }
            let row = if reads.engine.is_some() {
                reads
                    .reader
                    .lock()
                    .map_err(|_| "canonical read lock")?
                    .lookup(&request.origin, &request.identity)?
            } else {
                canonical_state(ledger, reads)?.lookup(&request.origin, &request.identity)?
            };
            if let Some(row) = &row {
                let observation = fabric::observation::decode_versioned(row.raw.as_bytes())?;
                if row.origin != request.origin
                    || row.source_id != request.source_id
                    || observation.identity() != &request.identity
                {
                    return Err("canonical receipt mismatch".into());
                }
            }
            Ok(Response::ExactReceiptFound {
                authority: reads.authority.clone(),
                row,
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
        Request::ReceiptBatchReady { request } => {
            if !reader || !allows(&request.origin) {
                return Err("unauthorized origin".into());
            }
            request.validate()?;
            let readiness =
                receipt_batch_readiness(&request, reads, projection, projection_error, query_lease);
            Ok(Response::ReceiptBatchReady { readiness })
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
        Request::IssueSegmentTicket {
            version,
            authority,
            origin,
            digest,
            start,
            end,
        } => {
            if !reader || reads.engine.is_some() || authority != reads.authority || !allows(&origin)
            {
                return Err("segment access unavailable".into());
            }
            let ticket = segment_ticket::issue(
                &reads.ticket_key,
                reads.ticket_epoch,
                version,
                authority,
                origin,
                digest,
                start,
                end,
            )?;
            Ok(Response::SegmentTicket { ticket })
        }
        Request::ReadSegmentChunk {
            ticket,
            offset,
            length,
        } => {
            if !reader
                || reads.engine.is_some()
                || ticket.authority != reads.authority
                || !allows(&ticket.origin)
            {
                return Err("segment access unavailable".into());
            }
            segment_ticket::verify(
                &reads.ticket_key,
                reads.ticket_epoch,
                &ticket,
                offset,
                length,
            )?;
            let cancel = reserve(reads, "segment-read", query_lease)?;
            let deadline = Instant::now() + Duration::from_secs(5);
            let projection = projection
                .try_lock()
                .map_err(|_| "projection segment unavailable")?;
            let bytes = projection.read_verified_segment_chunk(
                &fabric_query::transfer::SegmentChunkRequest {
                    active_authority: &reads.authority,
                    origin: &ticket.origin,
                    digest: &ticket.digest,
                    offset,
                    length,
                },
                &cancel,
                deadline,
            )?;
            Ok(Response::SegmentChunk { offset, bytes })
        }
        Request::Capabilities => Ok(Response::Capabilities {
            authority: reads.authority.clone(),
            schemas: vec![
                observation::VERSION,
                observation::VERSION_V3,
                observation::VERSION_V4,
                observation::VERSION_V5,
            ],
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
        Request::QueryWithOperationVersion { .. } => unreachable!("normalized above"),
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
    const USAGE: &str = "Usage: fabric-server serve CONFIG\nOptions: -h, --help";
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--help"] | ["-h"] => {
            println!("{USAGE}");
            Ok(())
        }
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
        _ => Err(USAGE.into()),
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
            decision_writers: vec![],
            node_quota_bytes: Default::default(),
            engine_endpoint: None,
            engine_auth_token_file: None,
            allow_unauthenticated_loopback_engine: false,
            max_connections: 4,
        })
        .unwrap();
        (root, server)
    }

    fn test_grant(
        server: &Server,
        credential: &str,
        request: &Request,
    ) -> handshake::AuthenticatedGrant {
        let auth = handshake::HandshakeAuth {
            reader: server.auth.reader.clone(),
            nodes: server.auth.nodes.clone(),
            writers: server.auth.writers.clone(),
            writer_bindings: server.auth.writer_bindings.clone(),
            authority: server.reads.authority.clone(),
        };
        let secret = if credential == "reader" {
            &auth.reader
        } else {
            auth.nodes.get(credential).unwrap()
        };
        let client_nonce = canonical::random_nonce_hex().unwrap();
        let server_nonce = canonical::random_nonce_hex().unwrap();
        let bytes = serde_json::to_vec(request).unwrap();
        let proof = canonical::build_client_proof(secret, &client_nonce, &server_nonce, &bytes);
        let peer = auth
            .verify_proof(&client_nonce, &server_nonce, &proof.proof)
            .unwrap();
        peer.verify_request(&client_nonce, &server_nonce, &bytes, &proof.request_mac)
            .unwrap()
    }

    fn call(server: &Server, credential: &str, request: Request) -> Result<Response, String> {
        let grant = test_grant(server, credential, &request);
        handle(
            &grant,
            request,
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
    fn rejected_operation_version_does_not_reserve_query_slot() {
        let (_root, server) = fixture();
        let request = QueryRequest {
            version: 1,
            query_id: "version-check".into(),
            authority: None,
            scope: vec!["not-granted".into()],
            snapshot: Default::default(),
            filter: Default::default(),
            operation: fabric::query::Operation::ListRuns,
            budgets: Default::default(),
            cursor: None,
        };
        let mut lease = None;
        let request = Request::QueryWithOperationVersion {
            request,
            operation_version: 2,
        };
        let grant = test_grant(&server, "reader", &request);
        let result = handle(
            &grant,
            request,
            &server.ledger,
            &server.projection,
            &server.auth,
            &server.projection_error,
            &server.reads,
            &mut lease,
        );
        assert!(matches!(result, Err(error) if error == "unsupported query operation contract"));
        assert!(lease.is_none());
        assert!(!server.reads.queries.lock().unwrap().active);
    }

    #[test]
    fn pressure_keeps_live_query_lease_through_response_write() {
        let (_root, server) = fixture();
        let request = Request::Plan {
            request: QueryRequest {
                version: fabric::query::API_VERSION,
                query_id: "one".into(),
                authority: None,
                scope: vec!["alpha".into()],
                snapshot: Default::default(),
                filter: Default::default(),
                operation: fabric::query::Operation::Evidence,
                budgets: Default::default(),
                cursor: None,
            },
        };
        let mut wrote = false;
        respond(
            Ok((test_grant(&server, "reader", &request), request)),
            &server.ledger,
            &server.projection,
            &server.auth,
            &server.projection_error,
            &server.reads,
            |body| {
                wrote = true;
                assert!(matches!(
                    serde_json::from_slice(body),
                    Ok(Response::Plan { .. })
                ));
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
                assert!(
                    server
                        .reads
                        .queries
                        .lock()
                        .unwrap()
                        .cancel
                        .load(Ordering::Acquire)
                );
                assert_eq!(
                    reserve(&server.reads, "two", &mut None).unwrap_err(),
                    "query busy"
                );
            },
        );
        assert!(wrote);
        assert_eq!(
            reserve(&server.reads, "two", &mut None).unwrap_err(),
            "analytical work deferred by resource pressure"
        );
        server.reads.pressure.store(false, Ordering::Release);
        assert!(reserve(&server.reads, "two", &mut None).is_ok());
    }

    #[test]
    fn socket_write_error_retains_query_lease_until_response_cleanup() {
        use std::net::{Shutdown, TcpListener, TcpStream};

        let (_root, server) = fixture();
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let _peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut socket, _) = listener.accept().unwrap();
        // A local write-half shutdown is deterministic and leaves the peer alive.
        socket.shutdown(Shutdown::Write).unwrap();
        let request = Request::Plan {
            request: QueryRequest {
                version: fabric::query::API_VERSION,
                query_id: "write-failure".into(),
                authority: None,
                scope: vec!["alpha".into()],
                snapshot: Default::default(),
                filter: Default::default(),
                operation: fabric::query::Operation::Evidence,
                budgets: Default::default(),
                cursor: None,
            },
        };
        let mut failure = None;
        respond(
            Ok((test_grant(&server, "reader", &request), request)),
            &server.ledger,
            &server.projection,
            &server.auth,
            &server.projection_error,
            &server.reads,
            |body| {
                assert!(matches!(
                    serde_json::from_slice(body),
                    Ok(Response::Plan { .. })
                ));
                assert_eq!(
                    reserve(&server.reads, "competing", &mut None).unwrap_err(),
                    "query busy"
                );
                failure = Some(
                    canonical::write_frame(
                        &mut socket,
                        body,
                        Instant::now() + Duration::from_secs(2),
                    )
                    .unwrap_err(),
                );
                assert_eq!(
                    reserve(&server.reads, "competing", &mut None).unwrap_err(),
                    "query busy"
                );
            },
        );
        assert!(failure.unwrap().starts_with("write; outcome unknown:"));
        let mut lease = None;
        assert!(reserve(&server.reads, "competing", &mut lease).is_ok());
        assert!(lease.is_some());
    }

    #[test]
    fn slow_tcp_reader_holds_query_lease_through_cancel_and_write_timeout() {
        use rustix::{
            event::{PollFd, PollFlags, Timespec, poll},
            net::sockopt,
        };
        use std::{
            net::{TcpListener, TcpStream},
            sync::mpsc,
        };

        let (_root, server) = fixture();
        let server = Arc::new(server);
        let mut rows = Vec::new();
        for id in 1..=25u64 {
            let raw = serde_json::json!({"version":2,"registry":"fabrico11y/2",
                "identity":{"producer":"worker","generation":"generation","sequence":id},
                "timestamp_ns":id,"resource":"local",
                "data":{"kind":"log","level":"info","message":"x".repeat(30_000)}})
            .to_string();
            let result = call(
                &server,
                "alpha",
                Request::Ingest {
                    origin: "alpha".into(),
                    records: vec![ForwardRecord {
                        source_id: id,
                        raw: raw.clone(),
                    }],
                },
            )
            .unwrap();
            assert!(matches!(result, Response::Committed { through, .. } if through == id));
            rows.push(canonical::CanonicalRow {
                canonical_id: id,
                origin: "alpha".into(),
                source_id: id,
                raw,
            });
        }
        server.projection.lock().unwrap().apply(&rows).unwrap();
        let request = Request::Query {
            request: QueryRequest {
                version: fabric::query::API_VERSION,
                query_id: "slow-reader".into(),
                authority: None,
                scope: vec!["alpha".into()],
                snapshot: Default::default(),
                filter: Default::default(),
                operation: fabric::query::Operation::Evidence,
                budgets: fabric::query::Budgets {
                    output_bytes: 1024 * 1024,
                    ..Default::default()
                },
                cursor: None,
            },
        };
        let grant = test_grant(&server, "reader", &request);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut socket, _) = listener.accept().unwrap();
        sockopt::set_socket_recv_buffer_size(&client, 4096).unwrap();
        sockopt::set_socket_send_buffer_size(&socket, 4096).unwrap();
        assert!(sockopt::socket_recv_buffer_size(&client).unwrap() <= 16384);
        assert!(sockopt::socket_send_buffer_size(&socket).unwrap() <= 16384);
        let observer = socket.try_clone().unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let owner = server.clone();
        let worker = thread::spawn(move || {
            let mut failure = None;
            respond(
                Ok((grant, request)),
                &owner.ledger,
                &owner.projection,
                &owner.auth,
                &owner.projection_error,
                &owner.reads,
                |body| {
                    assert!(matches!(
                        serde_json::from_slice(body),
                        Ok(Response::Query { .. })
                    ));
                    assert!(body.len() > 512 * 1024 && body.len() <= canonical::MAX_FRAME);
                    entered_tx.send(body.len()).unwrap();
                    failure = Some(
                        canonical::write_frame(
                            &mut socket,
                            body,
                            Instant::now() + Duration::from_secs(2),
                        )
                        .unwrap_err(),
                    );
                },
            );
            done_tx.send(failure.unwrap()).unwrap();
        });
        assert!(entered_rx.recv_timeout(Duration::from_secs(15)).unwrap() > 512 * 1024);
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let mut fds = [PollFd::new(&observer, PollFlags::OUT)];
            let ready = poll(
                &mut fds,
                Some(&Timespec {
                    tv_sec: 0,
                    tv_nsec: 10_000_000,
                }),
            )
            .unwrap();
            if ready == 0 {
                break;
            }
            assert!(Instant::now() < deadline, "sender never saturated");
        }
        assert!(matches!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        assert_eq!(
            reserve(&server.reads, "competing", &mut None).unwrap_err(),
            "query busy"
        );
        assert!(matches!(
            call(
                &server,
                "reader",
                Request::Cancel {
                    query_id: "slow-reader".into()
                }
            )
            .unwrap(),
            Response::Cancelled { .. }
        ));
        assert_eq!(
            reserve(&server.reads, "competing", &mut None).unwrap_err(),
            "query busy"
        );
        let error = done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            error.starts_with("write; outcome unknown:")
                || error.starts_with("deadline; outcome unknown"),
            "{error}"
        );
        worker.join().unwrap();
        let next = Request::Query {
            request: QueryRequest {
                version: fabric::query::API_VERSION,
                query_id: "next".into(),
                authority: None,
                scope: vec!["alpha".into()],
                snapshot: Default::default(),
                filter: Default::default(),
                operation: fabric::query::Operation::Evidence,
                budgets: fabric::query::Budgets {
                    rows: 1,
                    ..Default::default()
                },
                cursor: None,
            },
        };
        assert!(matches!(
            call(&server, "reader", next),
            Ok(Response::Query { .. })
        ));
        drop(client);
    }

    #[test]
    fn exact_receipt_lookup_binds_authority_role_origin_and_source() {
        let (_root, server) = fixture();
        let raw = serde_json::json!({"version":2,"registry":"fabrico11y/2", "identity":{"producer":"alpha","generation":"sample","sequence":1}, "timestamp_ns":0,"resource":"service","data":{"kind":"log","level":"info","message":"sample"}}).to_string();
        call(
            &server,
            "alpha",
            Request::Ingest {
                origin: "alpha".into(),
                records: vec![ForwardRecord {
                    source_id: 1,
                    raw: raw.clone(),
                }],
            },
        )
        .unwrap();
        let request = fabric::canonical::ExactReceiptRequest {
            authority: server.reads.authority.clone(),
            origin: "alpha".into(),
            source_id: 1,
            identity: observation::decode_versioned(raw.as_bytes())
                .unwrap()
                .identity()
                .clone(),
        };
        assert!(matches!(
            call(
                &server,
                "reader",
                Request::ExactReceiptLookup {
                    request: request.clone()
                }
            ),
            Ok(Response::ExactReceiptFound { row: Some(_), .. })
        ));
        assert!(
            call(
                &server,
                "alpha",
                Request::ExactReceiptLookup {
                    request: request.clone()
                }
            )
            .is_err()
        );
        let mut wrong = request.clone();
        wrong.authority = "other".into();
        assert!(
            call(
                &server,
                "reader",
                Request::ExactReceiptLookup { request: wrong }
            )
            .is_err()
        );
        let mut wrong = request.clone();
        wrong.origin = "not-granted".into();
        assert!(
            call(
                &server,
                "reader",
                Request::ExactReceiptLookup { request: wrong }
            )
            .is_err()
        );
        let mut wrong = request.clone();
        wrong.source_id = 2;
        assert!(
            call(
                &server,
                "reader",
                Request::ExactReceiptLookup { request: wrong }
            )
            .is_err()
        );
        let mut absent = request;
        absent.identity.sequence = 2;
        assert!(matches!(
            call(
                &server,
                "reader",
                Request::ExactReceiptLookup { request: absent }
            ),
            Ok(Response::ExactReceiptFound { row: None, .. })
        ));
    }

    #[test]
    fn receipt_batch_keeps_independent_scoped_outcomes() {
        use canonical::{ReceiptBatchItem, ReceiptBatchRequest, ReceiptBatchStatus as Status};
        let (_root, server) = fixture();
        let raw = |sequence: u64| {
            serde_json::json!({"version":2,"registry":"fabrico11y/2","identity":{"producer":"alpha","generation":"sample","sequence":sequence},"timestamp_ns":0,"resource":"service","data":{"kind":"log","level":"info","message":"sample"}}).to_string()
        };
        let entries: Vec<_> = (1..=2)
            .map(|id| ForwardRecord {
                source_id: id,
                raw: raw(id),
            })
            .collect();
        assert!(matches!(
            call(
                &server,
                "alpha",
                Request::Ingest {
                    origin: "alpha".into(),
                    records: entries
                }
            ),
            Ok(Response::Committed { through: 2, .. })
        ));
        let rows = server.ledger.lock().unwrap().replay(0, 2, 8192).unwrap();
        server.projection.lock().unwrap().apply(&rows[..1]).unwrap();
        let item = |id| ReceiptBatchItem {
            source_id: id,
            identity: observation::decode_versioned(raw(id).as_bytes())
                .unwrap()
                .identity()
                .clone(),
        };
        let mut mismatch = item(1);
        mismatch.identity.producer = "other".into();
        let request = ReceiptBatchRequest {
            authority: server.reads.authority.clone(),
            origin: "alpha".into(),
            source_barrier: 3,
            timeout_ms: 1000,
            include_digest: true,
            items: vec![item(1), item(2), mismatch, item(3), item(4), item(1)],
        };
        assert!(
            call(
                &server,
                "alpha",
                Request::ReceiptBatchReady {
                    request: request.clone()
                }
            )
            .is_err()
        );
        let mut denied = request.clone();
        denied.origin = "unknown".into();
        assert!(
            call(
                &server,
                "reader",
                Request::ReceiptBatchReady { request: denied }
            )
            .is_err()
        );
        let Response::ReceiptBatchReady { readiness } =
            call(&server, "reader", Request::ReceiptBatchReady { request }).unwrap()
        else {
            panic!("missing batch")
        };
        let statuses: Vec<_> = readiness.outcomes.iter().map(|x| x.status).collect();
        assert_eq!(
            statuses,
            vec![
                Status::Ready,
                Status::Pending,
                Status::Mismatch,
                Status::Missing,
                Status::Unavailable,
                Status::Ready
            ]
        );
        assert_eq!(readiness.segment_attempts, 1);
        assert_eq!(
            readiness.outcomes[0]
                .canonical_sha256
                .as_ref()
                .unwrap()
                .len(),
            64
        );
        assert_eq!(
            readiness.outcomes[0].canonical_sha256,
            readiness.outcomes[5].canonical_sha256
        );
    }

    #[test]
    fn receipt_batch_corrupt_canonical_row_does_not_hide_ready_peer() {
        use canonical::{ReceiptBatchItem, ReceiptBatchRequest, ReceiptBatchStatus as Status};
        let (root, server) = fixture();
        let raw = |sequence: u64| {
            serde_json::json!({"version":2,"registry":"fabrico11y/2","identity":{"producer":"alpha","generation":"sample","sequence":sequence},"timestamp_ns":0,"resource":"service","data":{"kind":"log","level":"info","message":"sample"}}).to_string()
        };
        let entries: Vec<_> = (1..=2)
            .map(|id| ForwardRecord {
                source_id: id,
                raw: raw(id),
            })
            .collect();
        assert!(matches!(
            call(
                &server,
                "alpha",
                Request::Ingest {
                    origin: "alpha".into(),
                    records: entries
                }
            ),
            Ok(Response::Committed { through: 2, .. })
        ));
        let rows = server.ledger.lock().unwrap().replay(0, 2, 8192).unwrap();
        server.projection.lock().unwrap().apply(&rows).unwrap();
        let db =
            rusqlite::Connection::open(root.path().join("canonical/canonical.sqlite")).unwrap();
        db.execute(
            "UPDATE records SET raw=42 WHERE origin='alpha' AND source_id=1",
            [],
        )
        .unwrap();
        let items = (1..=2)
            .map(|id| ReceiptBatchItem {
                source_id: id,
                identity: observation::decode_versioned(raw(id).as_bytes())
                    .unwrap()
                    .identity()
                    .clone(),
            })
            .collect();
        let request = ReceiptBatchRequest {
            authority: server.reads.authority.clone(),
            origin: "alpha".into(),
            source_barrier: 2,
            timeout_ms: 1000,
            include_digest: false,
            items,
        };
        let Response::ReceiptBatchReady { readiness } =
            call(&server, "reader", Request::ReceiptBatchReady { request }).unwrap()
        else {
            panic!("missing batch")
        };
        assert_eq!(readiness.outcomes[0].status, Status::Unavailable);
        assert_eq!(readiness.outcomes[1].status, Status::Ready);
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
