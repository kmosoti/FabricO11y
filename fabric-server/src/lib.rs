//! Bounded authoritative receiver and isolated analytical projection worker.
pub mod engine;
pub mod ledger;
use fabric::{
    canonical::{self, Authenticated, Request, Response},
    query::QueryRequest,
};
use fabric_query::Projection;
use ledger::Ledger;
use parking_lot::{FairMutex, FairMutexGuard};
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
        Arc, Mutex, RwLock,
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
    #[serde(default)]
    pub engine_endpoint: Option<String>,
    #[serde(default = "connections")]
    pub max_connections: usize,
}
fn connections() -> usize {
    16
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
    admission: FairMutex<()>,
    reader: Mutex<ledger::Reader>,
    authority: String,
    frontiers: RwLock<BTreeMap<String, u64>>,
    engine: Option<engine::Greptime>,
}
pub struct Server {
    listener: TcpListener,
    ledger: Arc<Mutex<Ledger>>,
    projection: Arc<Mutex<Projection>>,
    auth: Arc<Auth>,
    queries: Arc<Mutex<QuerySlot>>,
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
        let ledger = Arc::new(Mutex::new(Ledger::open_with_engine(
            &config.storage,
            config.quota_bytes,
            config.engine_endpoint.as_deref(),
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
        let reads = {
            let state = ledger.lock().map_err(|_| "canonical lock")?;
            Arc::new(ReadState {
                admission: FairMutex::new(()),
                reader: Mutex::new(state.reader()?),
                authority: state.authority.clone(),
                frontiers: RwLock::new(frontiers),
                engine: state.engine(),
            })
        };
        let listener = TcpListener::bind(address).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        Ok(Self {
            listener,
            ledger,
            projection,
            auth: Arc::new(Auth { reader, nodes }),
            queries: Arc::new(Mutex::new(QuerySlot {
                active: false,
                id: String::new(),
                cancel: Arc::new(AtomicBool::new(false)),
            })),
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
                    if native {
                        thread::sleep(Duration::from_millis(25));
                        continue;
                    }
                    if last.elapsed() < Duration::from_secs(1) {
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
                    if !rows.is_empty()
                        && let Err(err) = projection
                            .lock()
                            .map_err(|_| "projection lock")?
                            .apply(&rows)
                    {
                        if retry_projector_pressure(&err) {
                            thread::sleep(Duration::from_millis(25));
                            continue;
                        }
                        return Err(err);
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
            if projector.is_finished()
                && let Ok(mut fault) = self.projection_error.lock()
            {
                fault.get_or_insert_with(|| {
                    "analytical projection stopped; retained canonical records require recovery"
                        .to_string()
                });
            }
            match self.listener.accept() {
                Ok((mut socket, _)) => {
                    if active.load(Ordering::SeqCst) >= self.max_connections {
                        continue;
                    }
                    active.fetch_add(1, Ordering::SeqCst);
                    let guard = DropGuard::new(active.clone(), |active| {
                        active.fetch_sub(1, Ordering::SeqCst);
                    });
                    let ledger = self.ledger.clone();
                    let projection = self.projection.clone();
                    let auth = self.auth.clone();
                    let queries = self.queries.clone();
                    let projection_error = self.projection_error.clone();
                    let reads = self.reads.clone();
                    workers.push(thread::spawn(move || {
                        let _guard = guard;
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
                                &queries,
                                &projection_error,
                                &reads,
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
                    }));
                    workers.retain(|w| !w.is_finished());
                }
                Err(e) if retry_accept(&e) => thread::sleep(Duration::from_millis(5)),
                Err(e) => {
                    failure = Some(e.to_string());
                    break;
                }
            }
        }
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(q) = self.queries.lock() {
            q.cancel.store(true, Ordering::SeqCst)
        }
        for w in workers {
            let _ = w.join();
        }
        match projector.join() {
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err("projection worker panicked".into()),
            _ => (),
        }
        failure.map_or(Ok(()), Err)
    }
}
fn retry_accept(error: &std::io::Error) -> bool {
    use rustix::io::Errno;
    // Existing work can release transient pressure without abandoning custody.
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::ConnectionAborted
    ) || [Errno::MFILE, Errno::NFILE, Errno::NOBUFS, Errno::NOMEM]
        .iter()
        .any(|errno| error.raw_os_error() == Some(errno.raw_os_error()))
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

fn reserve(
    queries: &Arc<Mutex<QuerySlot>>,
    id: &str,
) -> Result<(Arc<AtomicBool>, QueryGuard), String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let mut slot = queries.lock().map_err(|_| "query lock")?;
    if slot.active {
        return Err("query busy".into());
    }
    slot.active = true;
    slot.id = id.to_owned();
    slot.cancel = cancel.clone();
    Ok((cancel, QueryGuard(queries.clone())))
}
fn handle(
    req: Authenticated,
    ledger: &Arc<Mutex<Ledger>>,
    projection: &Arc<Mutex<Projection>>,
    auth: &Auth,
    queries: &Arc<Mutex<QuerySlot>>,
    projection_error: &Mutex<Option<String>>,
    reads: &ReadState,
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
            let slot = queries.lock().map_err(|_| "query lock")?;
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
            let (cancel, _guard) = reserve(queries, &request.query_id)?;
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
            let (cancel, _guard) = reserve(queries, &request.query_id)?;
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
            use std::io::Read;
            let mut bytes = Vec::new();
            fs::File::open(config)
                .map_err(|e| e.to_string())?
                .take(65537)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > 65536 {
                return Err("configuration limit".into());
            }
            let server = Server::open(serde_json::from_slice(&bytes).map_err(|e| e.to_string())?)?;
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
            println!(
                "{}",
                serde_json::json!({"ready":true,"address":server.address()})
            );
            std::io::stdout().flush().map_err(|e| e.to_string())?;
            fabric::spindle::notify_systemd_ready();
            server.serve()
        }
        _ => Err("usage: fabric-server serve CONFIG".into()),
    }
}
