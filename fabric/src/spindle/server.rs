use super::{
    Config, Health, ORIGIN_MISMATCH, Reply, Request, SelfTelemetryHealth,
    custody::Store,
    pool::WorkerPool,
    sources::{self, FileSource, HostSource, JournalSource},
};
use crate::{
    observation::{self, Data, Level, MetricType, Outcome},
    sdk::Admission,
    self_telemetry::{SelfTelemetry, Service},
};
use rustix::{
    event::{EventfdFlags, PollFd, PollFlags, Timespec, eventfd, poll},
    net::sockopt::socket_peercred,
};
use std::{
    fs,
    io::{Read, Write},
    os::{
        fd::OwnedFd,
        unix::{
            fs::{FileTypeExt, MetadataExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    sync::Arc,
    time::{Duration, Instant},
};
struct Connection {
    stream: UnixStream,
    header: [u8; 4],
    header_used: usize,
    body: Vec<u8>,
    size: Option<usize>,
    output: Vec<u8>,
    written: usize,
    deadline: Instant,
    keep_alive: bool,
    remote: Option<std::sync::mpsc::Receiver<Result<Vec<u8>, String>>>,
}
enum ConnectionProgress {
    Pending,
    Request(Vec<u8>),
    Closed,
}
struct ForwardReceipt {
    authority: String,
    origin: String,
    through: u64,
    sent_through: u64,
}

const SELF_TELEMETRY_INTERVAL: Duration = Duration::from_secs(30);

struct SelfReporter {
    lane: SelfTelemetry,
    next_report: Instant,
    forwarding_failed: bool,
    commit_ns: Option<u64>,
    start_event: bool,
    start_log: bool,
}

impl SelfReporter {
    fn new(lane: SelfTelemetry) -> Self {
        Self {
            lane,
            next_report: Instant::now(),
            forwarding_failed: false,
            commit_ns: None,
            start_event: false,
            start_log: false,
        }
    }

    fn emit(&self, data: Data) -> bool {
        matches!(self.lane.try_emit(data), Admission::Accepted { .. })
    }

    fn health(&self) -> SelfTelemetryHealth {
        let status = self.lane.status();
        SelfTelemetryHealth {
            accepted: status.delivery.accepted,
            confirmed: status.delivery.confirmed,
            queued: status.delivery.charged_slots,
            dropped: status.dropped,
            blocked: status.delivery.last_error.is_some(),
        }
    }

    fn metric(&self, name: &str, unit: &str, value: u64, metric_type: MetricType) {
        self.lane.try_emit(Data::Metric {
            name: name.into(),
            unit: unit.into(),
            value: value as f64,
            metric_type,
            reset: false,
        });
    }

    fn report(&mut self, health: &Health, store: &Store, forwarding_enabled: bool) {
        self.next_report = Instant::now() + SELF_TELEMETRY_INTERVAL;
        if !self.start_event {
            self.start_event = self.emit(Data::Event {
                name: "spindle.self_telemetry.started".into(),
                message: "Spindle self-telemetry started".into(),
            });
        }
        if !self.start_log {
            self.start_log = self.emit(Data::Log {
                level: Level::Info,
                message: "Spindle reports to its configured telemetry lane".into(),
            });
        }
        for (name, value) in [
            ("spindle.ingestion.accepted", health.ingestion),
            ("spindle.ingestion.refused", health.refusals),
            ("spindle.ingestion.retries", health.retries),
            ("spindle.connections.errors", health.errors),
        ] {
            self.metric(name, "1", value, MetricType::Counter);
        }
        self.metric(
            "spindle.connections.active",
            "1",
            health.active_connections as u64,
            MetricType::Gauge,
        );
        self.metric(
            "spindle.connections.queue_bytes",
            "By",
            health.queue_bytes as u64,
            MetricType::Gauge,
        );
        self.metric(
            "spindle.self_telemetry.dropped",
            "1",
            self.health().dropped,
            MetricType::Counter,
        );
        if forwarding_enabled {
            if let (Ok(highest), Ok(forwarded)) = (store.highest_record_id(), store.forwarded()) {
                self.metric(
                    "spindle.forwarding.backlog",
                    "1",
                    highest.saturating_sub(forwarded),
                    MetricType::Gauge,
                );
            }
            let failed = health.forwarding_error.is_some();
            self.metric(
                "spindle.forwarding.failed",
                "1",
                u64::from(failed),
                MetricType::Gauge,
            );
            if self.forwarding_failed != failed {
                self.forwarding_failed = failed;
                let (name, message, level) = if failed {
                    (
                        "spindle.forwarding.failed",
                        "Spindle workload forwarding is blocked",
                        Level::Warn,
                    )
                } else {
                    (
                        "spindle.forwarding.recovered",
                        "Spindle workload forwarding recovered",
                        Level::Info,
                    )
                };
                self.lane.try_emit(Data::Event {
                    name: name.into(),
                    message: message.into(),
                });
                self.lane.try_emit(Data::Log {
                    level,
                    message: message.into(),
                });
            }
        }
        if let Some(duration_ns) = self.commit_ns {
            self.metric(
                "spindle.custody.commit_latency",
                "ns",
                duration_ns,
                MetricType::Gauge,
            );
            if self.emit(Data::ServiceRequest {
                service: "spindle".into(),
                operation: "custody.commit".into(),
                duration_ns,
                status: Outcome::Ok,
                trace_id: None,
            }) {
                self.commit_ns = None;
            }
        }
    }
}

pub struct Daemon {
    config: Config,
    store: Store,
    listener: UnixListener,
    connections: Vec<Connection>,
    connection_limit: usize,
    files: Vec<FileSource>,
    journal: Option<JournalSource>,
    host: Option<HostSource>,
    health: Health,
    start: Instant,
    stopping: bool,
    socket_identity: (u64, u64),
    next_source: usize,
    forward_worker: Option<std::sync::mpsc::Receiver<Result<ForwardReceipt, String>>>,
    next_forward: Instant,
    self_reporter: Option<SelfReporter>,
    wakeup: Arc<OwnedFd>,
    workers: WorkerPool,
}
impl Daemon {
    pub fn open(config: Config) -> Result<Self, String> {
        config.validate()?;
        let parent = config.socket.parent().ok_or("socket parent missing")?;
        let metadata = fs::metadata(parent).map_err(err)?;
        if metadata.uid() != rustix::process::getuid().as_raw()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err("socket parent must be private and owned by current UID".into());
        }
        let mut store = Store::open(&config.storage, config.quota_bytes)?;
        store.configure_forwarding(config.forwarding.clone())?;
        match fs::symlink_metadata(&config.socket) {
            Ok(m) => {
                if !m.file_type().is_socket() || m.uid() != rustix::process::getuid().as_raw() {
                    return Err("socket path is not an owned Unix socket".into());
                }
                let fd = rustix::net::socket_with(
                    rustix::net::AddressFamily::UNIX,
                    rustix::net::SocketType::STREAM,
                    rustix::net::SocketFlags::NONBLOCK | rustix::net::SocketFlags::CLOEXEC,
                    None,
                )
                .map_err(err)?;
                let addr = rustix::net::SocketAddrUnix::new(&config.socket).map_err(err)?;
                match rustix::net::connect(&fd, &addr) {
                    Err(e) if e == rustix::io::Errno::CONNREFUSED => {
                        fs::remove_file(&config.socket).map_err(err)?
                    }
                    _ => return Err("socket already live or inaccessible".into()),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(err(e)),
        }
        let listener = UnixListener::bind(&config.socket).map_err(err)?;
        fs::set_permissions(&config.socket, fs::Permissions::from_mode(0o600)).map_err(err)?;
        listener.set_nonblocking(true).map_err(err)?;
        let m = fs::metadata(&config.socket).map_err(err)?;
        let socket_identity = (m.dev(), m.ino());
        let files = config.files.iter().cloned().map(FileSource::new).collect();
        let journal = config.journal.clone().map(JournalSource::new);
        let host = config.host.clone().map(HostSource::new);
        let connection_limit = crate::resources::connection_limit(config.max_connections);
        let connections = Vec::with_capacity(connection_limit);
        let wakeup =
            Arc::new(eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK).map_err(err)?);
        let workers = WorkerPool::new("fabric-spindle", 4, 32, Arc::clone(&wakeup));
        Ok(Self {
            config,
            store,
            listener,
            connections,
            connection_limit,
            files,
            journal,
            host,
            health: Health {
                connection_limit: Some(connection_limit),
                ..Health::default()
            },
            start: Instant::now(),
            stopping: false,
            socket_identity,
            next_source: 0,
            forward_worker: None,
            next_forward: Instant::now(),
            self_reporter: None,
            wakeup,
            workers,
        })
    }

    /// Enables best-effort telemetry from the configured independent Spindle socket.
    pub fn enable_self_telemetry(&mut self) -> Result<(), String> {
        self.self_reporter = SelfTelemetry::from_env(Service::Spindle, Some(&self.config.socket))?
            .map(SelfReporter::new);
        Ok(())
    }

    pub fn run(&mut self) -> Result<(), String> {
        while !self.stopping || !self.connections.is_empty() {
            let ingestion_before = self.health.ingestion;
            self.tick()?;
            if self.stopping && self.connections.is_empty() {
                break;
            }
            // Successful admissions have durable replies queued for the next
            // tick; wait on fd readiness otherwise instead of sleep-polling.
            if self.health.ingestion == ingestion_before {
                self.wait_ready()?;
            }
        }
        Ok(())
    }

    fn wait_ready(&self) -> Result<(), String> {
        let now = Instant::now();
        let has_sources = !self.files.is_empty() || self.journal.is_some() || self.host.is_some();
        let mut wait = if has_sources {
            Duration::from_millis(25)
        } else {
            Duration::from_millis(250)
        };
        for c in &self.connections {
            wait = wait.min(c.deadline.saturating_duration_since(now));
        }
        if self.config.forwarding.is_some() && self.forward_worker.is_none() {
            wait = wait.min(self.next_forward.saturating_duration_since(now));
        }
        if let Some(reporter) = &self.self_reporter {
            wait = wait.min(reporter.next_report.saturating_duration_since(now));
        }

        let mut fds = Vec::with_capacity(2 + self.connections.len());
        fds.push(PollFd::new(&self.wakeup, PollFlags::IN));
        if !self.stopping {
            fds.push(PollFd::new(&self.listener, PollFlags::IN));
        }
        for c in &self.connections {
            if !c.output.is_empty() {
                fds.push(PollFd::new(&c.stream, PollFlags::OUT));
            } else if c.remote.is_none() {
                fds.push(PollFd::new(&c.stream, PollFlags::IN));
            }
        }

        let timeout = Timespec {
            tv_sec: wait.as_secs() as _,
            tv_nsec: wait.subsec_nanos() as _,
        };
        match poll(&mut fds, Some(&timeout)) {
            Ok(_) | Err(rustix::io::Errno::INTR) => {}
            Err(e) => return Err(err(e)),
        }
        let mut drain = [0u8; 8];
        let _ = rustix::io::read(&*self.wakeup, &mut drain);
        Ok(())
    }
    pub fn tick(&mut self) -> Result<(), String> {
        self.forward_tick();
        if !self.stopping {
            for _ in 0..self.connection_limit {
                match self.listener.accept() {
                    Ok((stream, _)) => {
                        if !admission_kernel::telemetry::elastic_can_admit(
                            self.connection_limit as u64,
                            self.connections.len() as u64,
                        ) {
                            self.health.refusals += 1;
                            continue;
                        }
                        let cred = socket_peercred(&stream).map_err(err)?;
                        if cred.uid != rustix::process::getuid() {
                            self.health.refusals += 1;
                            continue;
                        }
                        stream.set_nonblocking(true).map_err(err)?;
                        self.connections.push(Connection {
                            stream,
                            header: [0; 4],
                            header_used: 0,
                            body: Vec::new(),
                            size: None,
                            output: Vec::new(),
                            written: 0,
                            keep_alive: false,
                            remote: None,
                            deadline: Instant::now()
                                + Duration::from_millis(self.config.client_timeout_ms),
                        });
                    }
                    // Yield to existing connections so their replies or deadlines
                    // can return descriptors before the next accept attempt.
                    Err(e) if transient_accept_pressure(&e) => break,
                    Err(e) => return Err(err(e)),
                }
            }
        }
        if !self.connections.is_empty() {
            self.store.arm_connection_batch()?;
            let batch_started = Instant::now();
            let ingestion_before = self.health.ingestion;
            let retries_before = self.health.retries;
            let batch = self
                .progress_connections()
                .and_then(|()| self.store.commit_connection_batch_if_active().map(|_| ()));
            if let Err(error) = batch {
                let rollback = self.store.rollback_connection_batch();
                self.health.ingestion = ingestion_before;
                self.health.retries = retries_before;
                // Never leave rolled-back receipts available to a later tick, even
                // when an embedding caller catches the fatal error from tick().
                self.connections.clear();
                return Err(match rollback {
                    Ok(()) => error,
                    Err(rollback) => {
                        format!("{error}; custody batch rollback failed: {rollback}")
                    }
                });
            }
            if self.health.ingestion != ingestion_before {
                // Group commit latency includes the bounded synchronous batch and
                // its durable outer RELEASE, rather than only an inner savepoint.
                self.health.last_commit_latency_ns =
                    batch_started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
                if let Some(reporter) = &mut self.self_reporter {
                    reporter.commit_ns = Some(self.health.last_commit_latency_ns);
                }
            }
        }
        if !self.stopping {
            let total = self.files.len()
                + usize::from(self.journal.is_some())
                + usize::from(self.host.is_some());
            if total > 0 {
                let index = self.next_source % total;
                self.next_source = self.next_source.wrapping_add(1);
                if index < self.files.len() {
                    self.files[index].poll(&mut self.store)
                } else if index == self.files.len() && self.journal.is_some() {
                    self.journal
                        .as_mut()
                        .expect("journal")
                        .poll(&mut self.store)
                } else if let Some(host) = &mut self.host {
                    host.poll(&mut self.store)
                }
            }
            if let Some(reporter) = &mut self.self_reporter
                && Instant::now() >= reporter.next_report
            {
                self.health.active_connections = self.connections.len();
                self.health.queue_bytes = connection_queue_bytes(&self.connections);
                reporter.report(&self.health, &self.store, self.config.forwarding.is_some());
            }
        }
        Ok(())
    }
    fn progress_connections(&mut self) -> Result<(), String> {
        let client_timeout = Duration::from_millis(self.config.client_timeout_ms);
        let mut index = self.connections.len();
        while index > 0 {
            index -= 1;
            let mut connection = self.connections.swap_remove(index);
            let keep = if Instant::now() >= connection.deadline {
                self.health.refusals += 1;
                false
            } else {
                match connection.progress(client_timeout, self.stopping) {
                    Ok(ConnectionProgress::Request(body)) => {
                        match self.prepare_remote(&body) {
                            Ok(Some(job)) => {
                                let (send, receive) = std::sync::mpsc::sync_channel(1);
                                let deadline = connection.deadline;
                                match self.workers.try_submit(move || {
                                    let _ = send.send(job.execute(deadline));
                                }) {
                                    Ok(()) => {
                                        connection.remote = Some(receive);
                                        self.connections.push(connection);
                                        continue;
                                    }
                                    Err(error) => {
                                        connection.output =
                                            serde_json::to_vec(&Reply::Error { error })
                                                .map_err(err)?
                                    }
                                }
                            }
                            Ok(None) => {
                                let (reply, keep_alive) = self.handle_with_keep_alive(&body)?;
                                connection.output = reply;
                                if keep_alive {
                                    connection.keep_alive = true;
                                }
                            }
                            Err(error) => {
                                connection.output =
                                    serde_json::to_vec(&Reply::Error { error }).map_err(err)?
                            }
                        }
                        if connection.output.len() > super::MAX_REPLY {
                            connection.output = serde_json::to_vec(&Reply::Error {
                                error: "response budget exceeded; use a smaller page".into(),
                            })
                            .map_err(err)?;
                        }
                        let mut framed = Vec::with_capacity(connection.output.len() + 4);
                        framed.extend_from_slice(&(connection.output.len() as u32).to_be_bytes());
                        framed.append(&mut connection.output);
                        connection.output = framed;
                        // Newly prepared receipts (including leases) may only
                        // be written on the next tick, after outer RELEASE.
                        true
                    }
                    Ok(ConnectionProgress::Pending) => true,
                    Ok(ConnectionProgress::Closed) => false,
                    Err(_) => {
                        self.health.errors += 1;
                        false
                    }
                }
            };
            if keep {
                self.connections.push(connection)
            }
        }
        Ok(())
    }
    fn health(&mut self) -> Health {
        self.health.origin = self.store.origin().to_owned();
        self.health.canonical_through = self.store.forwarded().unwrap_or(0);
        self.health.reclaimed_through = self.store.reclaimed().unwrap_or(0);
        self.health.generation_capacity = self.store.generation_capacity().ok();
        self.health.records = self.store.count().unwrap_or(0);
        self.health.highest_record_id = self.store.highest_record_id().unwrap_or(0);
        self.health.custody_bytes = self.store.disk_bytes().unwrap_or(0);
        self.health.active_connections = self.connections.len();
        self.health.queue_bytes = connection_queue_bytes(&self.connections);
        self.health.uptime_ms = self.start.elapsed().as_millis() as u64;
        self.health.process_resident_bytes = sources::process_resident_bytes();
        self.health.self_telemetry = self.self_reporter.as_ref().map(SelfReporter::health);
        self.health.source_status = self.files.iter().map(|s| s.status.clone()).collect();
        if let Some(j) = &self.journal {
            self.health.source_status.push(j.status.clone())
        }
        if let Some(h) = &self.host {
            self.health.source_status.push(h.status.clone());
            self.health.missed_samples = h.missed
        }
        for status in &mut self.health.source_status {
            if status.state.len() > 1024 {
                let mut end = 1024;
                while !status.state.is_char_boundary(end) {
                    end -= 1
                }
                status.state.truncate(end);
            }
        }
        self.health.clone()
    }
    fn check_read_origin(&self, request: &Request) -> Result<(), String> {
        match request {
            Request::ReadForOrigin {
                expected_origin, ..
            } => self.check_origin(expected_origin),
            Request::ReadBatch {
                expected_origin: Some(expected_origin),
                ..
            } => self.check_origin(expected_origin),
            _ => Ok(()),
        }
    }
    fn check_origin(&self, expected_origin: &str) -> Result<(), String> {
        if expected_origin != self.store.origin() {
            return Err(ORIGIN_MISMATCH.to_owned());
        }
        Ok(())
    }
    fn lease_reply(&mut self, producer: &str, expected_origin: Option<&str>) -> Reply {
        match expected_origin
            .map_or(Ok(()), |origin| self.check_origin(origin))
            .and_then(|()| self.store.issue_lease(producer))
        {
            Ok((generation, expires_unix_s)) => Reply::Lease {
                generation,
                expires_unix_s,
            },
            Err(error) if error == "active generation limit" => {
                match self.store.generation_capacity() {
                    Ok(capacity) => Reply::GenerationCapacity {
                        used: capacity.used,
                        limit: capacity.limit,
                    },
                    Err(error) => Reply::Error { error },
                }
            }
            Err(error) => Reply::Error { error },
        }
    }
    fn prepare_remote(
        &mut self,
        bytes: &[u8],
    ) -> Result<Option<super::custody::RemoteJob>, String> {
        observation::check_json(bytes, observation::MAX_FRAME)?;
        let value: serde_json::Value = serde_json::from_slice(bytes).map_err(err)?;
        if value.get("command").is_some() {
            let request = serde_json::from_value(value).map_err(err)?;
            self.check_read_origin(&request)?;
            if let Request::Read { after, limit }
            | Request::ReadForOrigin { after, limit, .. }
            | Request::ReadBatch { after, limit, .. } = request
            {
                return self.store.prepare_remote_read(after, limit);
            }
            Ok(None)
        } else {
            if observation::decode_versioned(bytes)
                .is_ok_and(|o| observation::reserved_producer(&o.identity().producer))
            {
                return Err("collector producer namespace is reserved".into());
            }
            self.store.prepare_remote_retry(bytes)
        }
    }
    #[cfg(test)]
    fn handle(&mut self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        self.handle_with_keep_alive(bytes).map(|(reply, _)| reply)
    }
    fn handle_with_keep_alive(&mut self, bytes: &[u8]) -> Result<(Vec<u8>, bool), String> {
        let request = observation::check_json(bytes, observation::MAX_FRAME)
            .and_then(|()| serde_json::from_slice::<serde_json::Value>(bytes).map_err(err));
        let mut keep_alive = false;
        let reply = match request {
            Ok(v) if v.get("command").is_some() => match serde_json::from_value::<Request>(v) {
                Ok(Request::Status) => Reply::Status {
                    status: self.health(),
                },
                Ok(
                    request @ (Request::Read { after, limit }
                    | Request::ReadForOrigin { after, limit, .. }
                    | Request::ReadBatch { after, limit, .. }),
                ) => {
                    keep_alive = matches!(request, Request::ReadBatch { .. });
                    let bounded = limit.min(super::wire::MAX_READ_PAGE);
                    match self
                        .check_read_origin(&request)
                        .and_then(|()| self.store.read_raw(after, bounded))
                    {
                        Ok(records) => {
                            return Ok((super::wire::encode_records(&records)?, keep_alive));
                        }
                        Err(error) => Reply::Error { error },
                    }
                }
                Ok(Request::Lease { producer }) => self.lease_reply(&producer, None),
                Ok(Request::LeaseForOrigin {
                    producer,
                    expected_origin,
                }) => self.lease_reply(&producer, Some(&expected_origin)),
                Ok(Request::FabricProgress) => Reply::FabricProgress {
                    authority: self.config.forwarding.as_ref().map(|f| f.authority.clone()),
                    origin: self.store.origin().to_owned(),
                    through: self.store.forwarded()?,
                },
                Ok(Request::Stop) => {
                    self.stopping = true;
                    Reply::Stopped
                }
                Err(e) => Reply::Error { error: err(e) },
            },
            Ok(_) => {
                if observation::decode_versioned(bytes)
                    .is_ok_and(|o| observation::reserved_producer(&o.identity().producer))
                {
                    self.health.refusals += 1;
                    return Ok((
                        serde_json::to_vec(&Reply::Error {
                            error: "collector producer namespace is reserved".into(),
                        })
                        .map_err(err)?,
                        false,
                    ));
                }
                let result = self.store.submit(bytes, None);
                match result {
                    Ok(receipt) => {
                        self.health.ingestion += 1;
                        if receipt.duplicate {
                            self.health.retries += 1
                        }
                        Reply::Receipt { receipt }
                    }
                    Err(error) => {
                        self.health.refusals += 1;
                        Reply::Error { error }
                    }
                }
            }
            Err(error) => {
                self.health.errors += 1;
                Reply::Error { error }
            }
        };
        Ok((serde_json::to_vec(&reply).map_err(err)?, keep_alive))
    }
    fn forward_tick(&mut self) {
        use std::sync::mpsc::TryRecvError;
        // Confirmed custody can drain while a later forwarding RPC is waiting.
        // Each invocation still reclaims at most 32; failures retain the existing
        // retry backoff and do not discharge unconfirmed local payloads.
        if Instant::now() >= self.next_forward
            && let Err(error) = self.store.reclaim_confirmed()
        {
            self.health.forwarding_error = Some(error);
            self.next_forward = Instant::now() + Duration::from_millis(100);
            return;
        }
        if let Some(worker) = &self.forward_worker {
            match worker.try_recv() {
                Ok(result) => {
                    self.forward_worker = None;
                    let result = result.and_then(|receipt| {
                        self.store.acknowledge(
                            &receipt.authority,
                            &receipt.origin,
                            receipt.through,
                            receipt.sent_through,
                        )
                    });
                    self.health.forwarding_error = result.err();
                    self.next_forward = Instant::now()
                        + Duration::from_millis(if self.health.forwarding_error.is_some() {
                            100
                        } else {
                            0
                        });
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.forward_worker = None;
                    self.health.forwarding_error = Some("canonical worker disconnected".into());
                    self.next_forward = Instant::now() + Duration::from_millis(100);
                }
            }
        }
        if Instant::now() < self.next_forward {
            return;
        }
        let Some(config) = self.config.forwarding.clone() else {
            return;
        };
        let batch = match self.store.pending_batch() {
            Ok(batch) => batch,
            Err(error) => {
                self.health.forwarding_error = Some(error);
                return;
            }
        };
        let Some(last) = batch.last() else {
            return;
        };
        let sent = last.source_id;
        let origin = self.store.origin().to_owned();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        match self.workers.try_submit(move || {
            let result = config
                .credential()
                .and_then(|credential| {
                    crate::canonical::exchange(
                        &config.endpoint,
                        &credential,
                        &crate::canonical::Request::Ingest {
                            origin,
                            records: batch,
                        },
                        Instant::now() + Duration::from_secs(2),
                    )
                })
                .and_then(|response| match response {
                    crate::canonical::Response::Committed {
                        authority,
                        origin,
                        through,
                    } => Ok(ForwardReceipt {
                        authority,
                        origin,
                        through,
                        sent_through: sent,
                    }),
                    crate::canonical::Response::Error { error } => Err(error),
                    _ => Err("unexpected canonical receipt".into()),
                });
            let _ = send.send(result);
        }) {
            Ok(()) => self.forward_worker = Some(receive),
            Err(error) => self.health.forwarding_error = Some(error),
        }
    }
    pub fn socket(&self) -> &std::path::Path {
        &self.config.socket
    }
}

fn connection_queue_bytes(connections: &[Connection]) -> usize {
    connections
        .iter()
        .map(|connection| connection.body.capacity() + connection.output.capacity())
        .sum()
}
impl Drop for Daemon {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.config.socket)
            .is_ok_and(|m| (m.dev(), m.ino()) == self.socket_identity)
        {
            let _ = fs::remove_file(&self.config.socket);
        }
    }
}
impl Connection {
    fn progress(
        &mut self,
        client_timeout: Duration,
        stopping: bool,
    ) -> Result<ConnectionProgress, String> {
        if let Some(remote) = &self.remote {
            match remote.try_recv() {
                Ok(result) => {
                    self.remote = None;
                    let body = match result {
                        Ok(body) => body,
                        Err(error) => serde_json::to_vec(&Reply::Error { error }).map_err(err)?,
                    };
                    self.output
                        .extend_from_slice(&(body.len() as u32).to_be_bytes());
                    self.output.extend_from_slice(&body);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    return Ok(ConnectionProgress::Pending);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err("remote resolution worker disconnected".into());
                }
            }
        }
        if !self.output.is_empty() {
            return self.write_reply(client_timeout, stopping);
        }
        while self.header_used < 4 {
            match self.stream.read(&mut self.header[self.header_used..]) {
                Ok(0) if self.header_used == 0 => return Ok(ConnectionProgress::Closed),
                Ok(0) => return Err("truncated frame header".into()),
                Ok(n) => self.header_used += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    return Ok(ConnectionProgress::Pending);
                }
                Err(e) => return Err(err(e)),
            }
        }
        let size = *self
            .size
            .get_or_insert_with(|| u32::from_be_bytes(self.header) as usize);
        if size == 0 || size > observation::MAX_FRAME {
            return Err("invalid frame length".into());
        }
        // Allocate only after validating the advertised length.
        if self.body.is_empty() {
            self.body = Vec::with_capacity(size)
        }
        let mut chunk = [0; 8192];
        for _ in 0..8 {
            if self.body.len() == size {
                return Ok(ConnectionProgress::Request(std::mem::take(&mut self.body)));
            }
            let len = (size - self.body.len()).min(chunk.len());
            match self.stream.read(&mut chunk[..len]) {
                Ok(0) => return Err("truncated frame body".into()),
                Ok(n) => self.body.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    return Ok(ConnectionProgress::Pending);
                }
                Err(e) => return Err(err(e)),
            }
        }
        if self.body.len() == size {
            Ok(ConnectionProgress::Request(std::mem::take(&mut self.body)))
        } else {
            Ok(ConnectionProgress::Pending)
        }
    }
    fn write_reply(
        &mut self,
        client_timeout: Duration,
        stopping: bool,
    ) -> Result<ConnectionProgress, String> {
        for _ in 0..8 {
            match self.stream.write(&self.output[self.written..]) {
                Ok(0) => return Err("reply write returned zero".into()),
                Ok(n) => {
                    self.written += n;
                    if self.written == self.output.len() {
                        if self.keep_alive && !stopping {
                            self.header = [0; 4];
                            self.header_used = 0;
                            self.body.clear();
                            self.size = None;
                            self.output.clear();
                            self.written = 0;
                            self.deadline = Instant::now() + client_timeout;
                            return Ok(ConnectionProgress::Pending);
                        }
                        return Ok(ConnectionProgress::Closed);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    return Ok(ConnectionProgress::Pending);
                }
                Err(e) => return Err(err(e)),
            }
        }
        Ok(ConnectionProgress::Pending)
    }
}

fn transient_accept_pressure(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::ConnectionAborted
    ) || crate::resources::is_resource_error(error)
}

#[cfg(test)]
#[test]
fn retryable_admission_pressure_preserves_listener_progress() {
    use rustix::io::Errno;
    for errno in [
        Errno::AGAIN,
        Errno::MFILE,
        Errno::NFILE,
        Errno::NOBUFS,
        Errno::NOMEM,
    ] {
        assert!(transient_accept_pressure(&errno.into()));
    }
    assert!(!transient_accept_pressure(&Errno::BADF.into()));
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[cfg(test)]
mod remote_isolation_tests {
    use super::*;
    use std::{net::TcpListener, sync::mpsc};
    #[test]
    fn stalled_reclaimed_read_does_not_block_new_local_custody() {
        let temp = tempfile::tempdir().unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let credential = temp.path().join("credential");
        fs::write(&credential, "fixture-key").unwrap();
        fs::set_permissions(&credential, fs::Permissions::from_mode(0o600)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let config = Config {
            socket: temp.path().join("socket"),
            storage: temp.path().join("store"),
            quota_bytes: 2 * 1024 * 1024,
            forwarding: Some(super::super::ForwardConfig {
                endpoint: listener.local_addr().unwrap().to_string(),
                credential_file: credential,
                authority: "authority".into(),
                reclaim_confirmed: true,
            }),
            max_connections: 4,
            client_timeout_ms: 1000,
            files: vec![],
            journal: None,
            host: None,
        };
        let mut daemon = Daemon::open(config.clone()).unwrap();
        let raw=br#"{"version":2,"registry":"fabrico11y/2","identity":{"producer":"test","generation":"legacy","sequence":1},"timestamp_ns":1,"resource":"fixture","data":{"kind":"event","name":"test","message":"one"}}"#;
        daemon.store.submit(raw, None).unwrap();
        let origin = daemon.store.origin().to_owned();
        daemon
            .store
            .acknowledge("authority", &origin, 1, 1)
            .unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let remote = std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        });
        let path = config.socket.clone();
        let read = std::thread::spawn(move || {
            super::super::transport::request(
                &path,
                &Request::Read { after: 0, limit: 1 },
                Instant::now() + Duration::from_secs(2),
            )
        });
        let end = Instant::now() + Duration::from_secs(2);
        while started_rx.try_recv().is_err() {
            daemon.tick().unwrap();
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
        let next = String::from_utf8(raw.to_vec())
            .unwrap()
            .replace("\"sequence\":1", "\"sequence\":2");
        let path = config.socket.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let submission = std::thread::spawn(move || {
            done_tx
                .send(super::super::transport::submit_bytes(
                    &path,
                    next.as_bytes(),
                    Instant::now() + Duration::from_secs(1),
                ))
                .unwrap();
        });
        let start = Instant::now();
        loop {
            daemon.tick().unwrap();
            if let Ok(result) = done_rx.try_recv() {
                assert_eq!(result.unwrap().id, 2);
                break;
            }
            assert!(
                start.elapsed() < Duration::from_millis(250),
                "remote resolution blocked local custody"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        release_tx.send(()).unwrap();
        remote.join().unwrap();
        submission.join().unwrap();
        let end = Instant::now() + Duration::from_secs(2);
        while !read.is_finished() {
            daemon.tick().unwrap();
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
        let result = read.join().unwrap();
        assert!(matches!(result, Ok(Reply::Error { .. }) | Err(_)));
    }
}

#[cfg(test)]
mod shadow_reclamation_tests {
    use super::*;
    use crate::canonical::{self, CanonicalRow, Response};
    use std::{
        collections::BTreeMap,
        net::TcpListener,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };
    #[test]
    fn shadow_confirmation_survives_restart_then_opt_in_reclaims_without_new_records() {
        let temp = tempfile::tempdir().unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let credential = temp.path().join("key");
        fs::write(&credential, "fixture-key").unwrap();
        fs::set_permissions(&credential, fs::Permissions::from_mode(0o600)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let forwarding:super::super::ForwardConfig=serde_json::from_value(serde_json::json!({"endpoint":listener.local_addr().unwrap().to_string(),"credential_file":credential,"authority":"authority"})).unwrap();
        assert!(!forwarding.reclaim_confirmed);
        let retained = Arc::new(Mutex::new(BTreeMap::<u64, CanonicalRow>::new()));
        let remote = retained.clone();
        let reads = Arc::new(AtomicUsize::new(0));
        let remote_reads = reads.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let exit = stop.clone();
        let authority = std::thread::spawn(move || {
            while !exit.load(Ordering::Relaxed) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                let deadline = Instant::now() + Duration::from_secs(2);
                let (_, request) =
                    canonical::accept_authenticated_sync(&mut stream, ["fixture-key"], deadline)
                        .unwrap();
                let response = match request {
                    canonical::Request::Ingest { origin, records } => {
                        let mut stored = remote.lock().unwrap();
                        for record in records {
                            if let Some(previous) = stored.get(&record.source_id) {
                                assert_eq!(previous.raw, record.raw);
                            } else {
                                stored.insert(
                                    record.source_id,
                                    CanonicalRow {
                                        canonical_id: record.source_id,
                                        origin: origin.clone(),
                                        source_id: record.source_id,
                                        raw: record.raw,
                                    },
                                );
                            }
                        }
                        Response::Committed {
                            authority: "authority".into(),
                            origin,
                            through: stored.keys().copied().max().unwrap_or(0),
                        }
                    }
                    canonical::Request::Read {
                        origin,
                        after,
                        through,
                        limit,
                    } => {
                        remote_reads.fetch_add(1, Ordering::Relaxed);
                        let stored = remote.lock().unwrap();
                        Response::Rows {
                            rows: stored
                                .values()
                                .filter(|row| {
                                    row.origin == origin
                                        && row.source_id > after
                                        && row.source_id <= through
                                })
                                .take(limit)
                                .cloned()
                                .collect(),
                        }
                    }
                    canonical::Request::Lookup { origin, identity } => {
                        let stored = remote.lock().unwrap();
                        Response::Found {
                            row: stored
                                .values()
                                .find(|row| {
                                    row.origin == origin
                                        && observation::decode_versioned(row.raw.as_bytes())
                                            .unwrap()
                                            .identity()
                                            == &identity
                                })
                                .cloned(),
                        }
                    }
                    _ => Response::Error {
                        error: "unsupported fixture command".into(),
                    },
                };
                canonical::write_frame(
                    &mut stream,
                    &serde_json::to_vec(&response).unwrap(),
                    deadline,
                )
                .unwrap();
            }
        });
        let mut config = Config {
            socket: temp.path().join("socket"),
            storage: temp.path().join("store"),
            quota_bytes: 2 * 1024 * 1024,
            forwarding: Some(forwarding),
            max_connections: 4,
            client_timeout_ms: 1000,
            files: vec![],
            journal: None,
            host: None,
        };
        let raw=br#"  {"version":2,"registry":"fabrico11y/2","identity":{"producer":"shadow","generation":"legacy","sequence":1},"timestamp_ns":1,"resource":"fixture","data":{"kind":"event","name":"test","message":"retained original bytes"}} "#;
        let mut daemon = Daemon::open(config.clone()).unwrap();
        assert_eq!(daemon.store.submit(raw, None).unwrap().id, 1);
        let end = Instant::now() + Duration::from_secs(3);
        while daemon.store.forwarded().unwrap() != 1 {
            daemon.tick().unwrap();
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(daemon.store.count().unwrap(), 1);
        assert_eq!(daemon.store.reclaimed().unwrap(), 0);
        assert_eq!(daemon.store.read_raw(0, 1).unwrap()[0].1, raw);
        assert_eq!(retained.lock().unwrap()[&1].raw.as_bytes(), raw);
        let origin = daemon.store.origin().to_owned();
        let expected_read = |origin: &str| {
            serde_json::to_vec(&Request::ReadForOrigin {
                expected_origin: origin.to_owned(),
                after: 0,
                limit: 1,
            })
            .unwrap()
        };
        assert!(
            daemon
                .prepare_remote(&expected_read(&origin))
                .unwrap()
                .is_none()
        );
        let local = daemon.handle(&expected_read(&origin)).unwrap();
        assert_eq!(
            super::super::wire::decode_records_raw(&local).unwrap()[0].1,
            raw
        );
        let wrong_local = daemon.handle(&expected_read("other-store")).unwrap();
        assert_eq!(
            super::super::wire::decode_records_raw(&wrong_local).unwrap_err(),
            ORIGIN_MISMATCH,
        );
        drop(daemon);
        let mut shadow = Daemon::open(config.clone()).unwrap();
        assert_eq!(shadow.store.origin(), origin);
        assert_eq!(shadow.store.forwarded().unwrap(), 1);
        assert_eq!(shadow.store.reclaimed().unwrap(), 0);
        assert_eq!(shadow.store.read_raw(0, 1).unwrap()[0].1, raw);
        assert_eq!(
            reads.load(Ordering::Relaxed),
            0,
            "shadow reads must remain local"
        );
        shadow.tick().unwrap();
        assert_eq!(shadow.store.count().unwrap(), 1);
        drop(shadow);
        config.forwarding.as_mut().unwrap().reclaim_confirmed = true;
        let mut enabled = Daemon::open(config.clone()).unwrap();
        assert!(enabled.store.pending_batch().unwrap().is_empty());
        enabled.tick().unwrap();
        assert_eq!(enabled.store.count().unwrap(), 0);
        assert_eq!(enabled.store.forwarded().unwrap(), 1);
        assert_eq!(enabled.store.reclaimed().unwrap(), 1);
        assert_eq!(enabled.store.highest_record_id().unwrap(), 1);
        let socket = config.socket.clone();
        let actual_origin = origin.clone();
        let read = std::thread::spawn(move || {
            use super::super::transport;
            let deadline = Instant::now() + Duration::from_secs(2);
            assert_eq!(
                transport::read_origin(&socket, deadline).unwrap(),
                actual_origin
            );
            assert_eq!(
                transport::read_raw_for_origin(&socket, "other-store", 0, deadline).unwrap_err(),
                ORIGIN_MISMATCH,
            );
            transport::read_raw_for_origin(&socket, &actual_origin, 0, deadline)
        });
        let end = Instant::now() + Duration::from_secs(3);
        while !read.is_finished() {
            enabled.tick().unwrap();
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(read.join().unwrap().unwrap()[0].1, raw);
        assert_eq!(reads.load(Ordering::Relaxed), 1);
        let socket = config.socket.clone();
        let retry = std::thread::spawn(move || {
            super::super::transport::submit_bytes(
                &socket,
                raw,
                Instant::now() + Duration::from_secs(2),
            )
        });
        let end = Instant::now() + Duration::from_secs(3);
        while !retry.is_finished() {
            enabled.tick().unwrap();
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(retry.join().unwrap().unwrap().duplicate);
        assert!(
            enabled
                .store
                .configure_forwarding(None)
                .unwrap_err()
                .contains("cannot be disabled")
        );
        let mut rollback = config.forwarding.clone().unwrap();
        rollback.reclaim_confirmed = false;
        enabled.store.configure_forwarding(Some(rollback)).unwrap();
        assert_eq!(enabled.store.read_raw(0, 1).unwrap()[0].1, raw);
        let next = String::from_utf8(raw.to_vec())
            .unwrap()
            .replace("\"sequence\":1", "\"sequence\":2");
        assert_eq!(enabled.store.submit(next.as_bytes(), None).unwrap().id, 2);
        drop(enabled);
        stop.store(true, Ordering::Relaxed);
        authority.join().unwrap();
    }
}

#[cfg(test)]
mod batch_publication_tests {
    use super::*;
    fn config(root: &std::path::Path) -> Config {
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
        Config {
            socket: root.join("socket"),
            storage: root.join("store"),
            quota_bytes: 2 * 1024 * 1024,
            forwarding: None,
            max_connections: 4,
            client_timeout_ms: 1000,
            files: vec![],
            journal: None,
            host: None,
        }
    }
    fn submission(config: &Config) -> UnixStream {
        let raw = br#"{"version":2,"registry":"fabrico11y/2","identity":{"producer":"batch","generation":"legacy","sequence":1},"timestamp_ns":1,"resource":"fixture","data":{"kind":"event","name":"batch","message":"one"}}"#;
        let mut client = UnixStream::connect(&config.socket).unwrap();
        client.write_all(&(raw.len() as u32).to_be_bytes()).unwrap();
        client.write_all(raw).unwrap();
        client.set_nonblocking(true).unwrap();
        client
    }
    #[test]
    fn prepared_receipt_waits_for_commit_and_is_only_written_on_following_tick() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let mut daemon = Daemon::open(config.clone()).unwrap();
        let mut client = submission(&config);
        daemon.tick().unwrap();
        assert_eq!(daemon.store.count().unwrap(), 1);
        let mut header = [0; 4];
        assert_eq!(
            client.read(&mut header).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        daemon.tick().unwrap();
        assert_eq!(daemon.health.errors, 0);
        client.set_nonblocking(false).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        client.read_exact(&mut header).unwrap();
        let mut body = vec![0; u32::from_be_bytes(header) as usize];
        client.read_exact(&mut body).unwrap();
        assert!(
            matches!(serde_json::from_slice::<Reply>(&body).unwrap(), Reply::Receipt { receipt } if receipt.id == 1)
        );
        drop(daemon);
        assert_eq!(
            Store::open(&config.storage, config.quota_bytes)
                .unwrap()
                .count()
                .unwrap(),
            1
        );
    }
    #[test]
    fn failed_outer_commit_closes_queued_receipts_and_rolls_back_custody() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let mut daemon = Daemon::open(config.clone()).unwrap();
        daemon.store.inject_batch_commit_failure();
        let mut client = submission(&config);
        assert!(daemon.tick().unwrap_err().contains("FOREIGN KEY"));
        assert!(daemon.connections.is_empty());
        assert_eq!(daemon.health.ingestion, 0);
        assert_eq!(daemon.health.retries, 0);
        let mut header = [0; 4];
        assert_eq!(client.read(&mut header).unwrap(), 0);
        // Even a caller that catches the fatal tick error cannot publish a
        // rolled-back receipt on a subsequent tick.
        daemon.tick().unwrap();
        assert_eq!(daemon.health.ingestion, 0);
        assert_eq!(daemon.health.retries, 0);
        drop(daemon);
        let store = Store::open(&config.storage, config.quota_bytes).unwrap();
        assert_eq!(store.count().unwrap(), 0);
        assert_eq!(store.highest_record_id().unwrap(), 0);
    }

    #[test]
    fn self_telemetry_uses_separate_store_without_delaying_workload_custody() {
        let temp = tempfile::Builder::new()
            .prefix("spindle-melt-")
            .tempdir_in(std::env::var_os("TMPDIR").unwrap_or_else(|| "/var/tmp".into()))
            .unwrap();
        let lane_path = temp.path().join("lane");
        fs::create_dir(&lane_path).unwrap();
        let lane_config = config(&lane_path);
        let workload_config = config(temp.path());
        let mut daemon = Daemon::open(workload_config.clone()).unwrap();
        let lane =
            SelfTelemetry::open(Service::Spindle, &lane_config.socket, Some(daemon.socket()))
                .unwrap();
        daemon.self_reporter = Some(SelfReporter::new(lane));
        let mut client = submission(&workload_config);

        // The telemetry socket is absent. Workload storage and receipt
        // publication must still advance independently on consecutive ticks.
        daemon.tick().unwrap();
        daemon.tick().unwrap();
        client.set_nonblocking(false).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut header = [0; 4];
        client.read_exact(&mut header).unwrap();
        let mut body = vec![0; u32::from_be_bytes(header) as usize];
        client.read_exact(&mut body).unwrap();
        assert!(matches!(
            serde_json::from_slice::<Reply>(&body).unwrap(),
            Reply::Receipt { receipt } if receipt.id == 1
        ));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !daemon.health().self_telemetry.unwrap().blocked {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }

        // Restore the dedicated store and drive both daemons. If a nonblocking
        // SDK admission contends during a sample, a later sample retries.
        let mut sink = Daemon::open(lane_config).unwrap();
        let mut found = [false; 4];
        let mut after = 0;
        let deadline = Instant::now() + Duration::from_secs(3);
        while found.contains(&false) {
            sink.tick().unwrap();
            let status = daemon.health().self_telemetry.unwrap();
            if status.queued == 0 {
                daemon.self_reporter.as_mut().unwrap().next_report = Instant::now();
            }
            daemon.tick().unwrap();
            while let Some((id, raw)) = sink.store.read_raw(after, 1).unwrap().pop() {
                after = id;
                let record = observation::decode_versioned(&raw).unwrap();
                assert_eq!(record.identity().producer, "spindle.self");
                match record.data() {
                    Data::Metric { .. } => found[0] = true,
                    Data::Event { .. } => found[1] = true,
                    Data::Log { .. } => found[2] = true,
                    Data::ServiceRequest { operation, .. } if operation == "custody.commit" => {
                        found[3] = true;
                    }
                    data => panic!("unexpected self-telemetry: {data:?}"),
                }
            }
            assert!(Instant::now() < deadline, "missing signal types: {found:?}");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(daemon.store.highest_record_id().unwrap(), 1);
        let lane_origin = sink.store.origin().to_owned();
        assert_ne!(daemon.store.origin(), lane_origin);
        assert!(sink.health().self_telemetry.is_none());
        drop(daemon.self_reporter.take());
        drop(sink);
        let reopened = Store::open(&lane_path.join("store"), 2 * 1024 * 1024).unwrap();
        assert_eq!(reopened.origin(), lane_origin);
        assert!(reopened.highest_record_id().unwrap() >= after);
    }
}

#[cfg(test)]
mod connection_accounting_tests {
    use super::*;
    use std::net::Shutdown;

    fn config(root: &std::path::Path) -> Config {
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
        Config {
            socket: root.join("socket"),
            storage: root.join("store"),
            quota_bytes: 2 * 1024 * 1024,
            forwarding: None,
            max_connections: 4,
            client_timeout_ms: 1000,
            files: vec![],
            journal: None,
            host: None,
        }
    }
    #[test]
    fn qualified_lease_refuses_wrong_origin_before_allocating() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let mut daemon = Daemon::open(config.clone()).unwrap();
        let expected_origin = daemon.store.origin().to_owned();
        let wrong = serde_json::to_vec(&Request::LeaseForOrigin {
            producer: "rejected".into(),
            expected_origin: "different-origin".into(),
        })
        .unwrap();
        assert!(matches!(
            serde_json::from_slice::<Reply>(&daemon.handle(&wrong).unwrap()).unwrap(),
            Reply::Error { error } if error == ORIGIN_MISMATCH
        ));
        let correct = serde_json::to_vec(&Request::LeaseForOrigin {
            producer: "accepted".into(),
            expected_origin,
        })
        .unwrap();
        assert!(matches!(
            serde_json::from_slice::<Reply>(&daemon.handle(&correct).unwrap()).unwrap(),
            Reply::Lease { .. }
        ));
        drop(daemon);
        let conn = rusqlite::Connection::open_with_flags(
            config.storage.join("custody.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert_eq!(
            conn.query_row("SELECT producer FROM generations", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "accepted"
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM generations", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            1
        );
    }

    fn status_request(config: &Config) -> UnixStream {
        let mut client = UnixStream::connect(&config.socket).unwrap();
        let body = serde_json::to_vec(&Request::Status).unwrap();
        client
            .write_all(&(body.len() as u32).to_be_bytes())
            .unwrap();
        client.write_all(&body).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        client
    }
    #[test]
    fn complete_one_shot_control_reply_and_empty_peer_eof_are_normal_closures() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let mut daemon = Daemon::open(config.clone()).unwrap();
        let mut client = status_request(&config);
        daemon.tick().unwrap();
        assert_eq!(daemon.connections.len(), 1);
        daemon.tick().unwrap();
        let mut header = [0; 4];
        client.read_exact(&mut header).unwrap();
        let mut body = vec![0; u32::from_be_bytes(header) as usize];
        client.read_exact(&mut body).unwrap();
        assert!(matches!(
            serde_json::from_slice::<Reply>(&body).unwrap(),
            Reply::Status { .. }
        ));
        assert_eq!(client.read(&mut header).unwrap(), 0);
        assert!(daemon.connections.is_empty());
        assert_eq!(daemon.health.errors, 0);
        drop(client);
        // A probe can connect and close without ever starting a framed request.
        drop(UnixStream::connect(&config.socket).unwrap());
        daemon.tick().unwrap();
        assert!(daemon.connections.is_empty());
        assert_eq!(daemon.health.errors, 0);
        assert_eq!(daemon.health.refusals, 0);
    }
    #[test]
    fn truncated_headers_bodies_and_invalid_lengths_still_increment_errors() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let mut daemon = Daemon::open(config.clone()).unwrap();
        let mut truncated_body = 8u32.to_be_bytes().to_vec();
        truncated_body.extend_from_slice(b"{}");
        let cases = [
            ("truncated header", vec![0, 0]),
            ("truncated body", truncated_body),
            ("zero length", 0u32.to_be_bytes().to_vec()),
            (
                "oversized length",
                (observation::MAX_FRAME as u32 + 1).to_be_bytes().to_vec(),
            ),
        ];
        for (index, (label, bytes)) in cases.into_iter().enumerate() {
            let mut client = UnixStream::connect(&config.socket).unwrap();
            client.write_all(&bytes).unwrap();
            client.shutdown(Shutdown::Write).unwrap();
            daemon.tick().unwrap();
            assert_eq!(daemon.health.errors, index as u64 + 1, "{label}");
            assert!(daemon.connections.is_empty(), "{label}");
            assert_eq!(daemon.store.count().unwrap(), 0);
        }
        assert_eq!(daemon.health.refusals, 0);
    }
    #[test]
    fn native_broken_reply_write_is_an_error_not_a_completed_response() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let mut daemon = Daemon::open(config.clone()).unwrap();
        let client = status_request(&config);
        daemon.tick().unwrap();
        assert_eq!(daemon.connections.len(), 1);
        assert!(!daemon.connections[0].output.is_empty());
        assert_eq!(daemon.connections[0].written, 0);
        assert_eq!(daemon.health.errors, 0);
        // The kernel rejects the actual response write after peer shutdown.
        client.shutdown(Shutdown::Both).unwrap();
        daemon.tick().unwrap();
        assert!(daemon.connections.is_empty());
        assert_eq!(daemon.health.errors, 1);
        assert_eq!(daemon.health.refusals, 0);
    }
    #[test]
    fn completed_remote_job_reply_closes_without_counting_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let mut daemon = Daemon::open(config).unwrap();
        let (server, mut client) = UnixStream::pair().unwrap();
        server.set_nonblocking(true).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let response = serde_json::to_vec(&Reply::Stopped).unwrap();
        send.send(Ok(response.clone())).unwrap();
        daemon.connections.push(Connection {
            stream: server,
            header: [0; 4],
            header_used: 0,
            body: Vec::new(),
            size: None,
            output: Vec::new(),
            written: 0,
            deadline: Instant::now() + Duration::from_secs(1),
            keep_alive: false,
            remote: Some(receive),
        });
        daemon.tick().unwrap();
        let mut header = [0; 4];
        client.read_exact(&mut header).unwrap();
        assert_eq!(u32::from_be_bytes(header) as usize, response.len());
        let mut body = vec![0; response.len()];
        client.read_exact(&mut body).unwrap();
        assert_eq!(body, response);
        assert_eq!(client.read(&mut header).unwrap(), 0);
        assert!(daemon.connections.is_empty());
        assert_eq!(daemon.health.errors, 0);
    }
}

#[cfg(test)]
mod forwarding_reclamation_tests {
    use super::*;
    use std::{net::TcpListener, sync::mpsc};
    fn raw(sequence: u64) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"version":2,"registry":"fabrico11y/2","identity":{"producer":"gc-forward","generation":"legacy","sequence":sequence},"timestamp_ns":1,"resource":"fixture","data":{"kind":"event","name":"gc","message":"retained exact bytes"}})).unwrap()
    }
    #[test]
    fn confirmed_gc_advances_during_later_blocked_forward_and_admission_remains_usable() {
        let temp = tempfile::tempdir().unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let credential = temp.path().join("credential");
        fs::write(&credential, "fixture-key").unwrap();
        fs::set_permissions(&credential, fs::Permissions::from_mode(0o600)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let config = Config {
            socket: temp.path().join("socket"),
            storage: temp.path().join("store"),
            quota_bytes: 4 * 1024 * 1024,
            forwarding: Some(super::super::ForwardConfig {
                endpoint: listener.local_addr().unwrap().to_string(),
                credential_file: credential,
                authority: "authority".into(),
                reclaim_confirmed: true,
            }),
            max_connections: 4,
            client_timeout_ms: 1000,
            files: vec![],
            journal: None,
            host: None,
        };
        let mut daemon = Daemon::open(config.clone()).unwrap();
        daemon.store.begin_connection_batch().unwrap();
        for sequence in 1..=129 {
            daemon.store.submit(&raw(sequence), None).unwrap();
        }
        daemon.store.commit_connection_batch().unwrap();
        let origin = daemon.store.origin().to_owned();
        daemon
            .store
            .acknowledge("authority", &origin, 128, 128)
            .unwrap();
        assert_eq!(daemon.store.reclaimed().unwrap(), 32);

        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let remote_origin = origin.clone();
        let remote = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (_, request) = crate::canonical::accept_authenticated_sync(
                &mut stream,
                ["fixture-key"],
                Instant::now() + Duration::from_secs(2),
            )
            .unwrap();
            match request {
                crate::canonical::Request::Ingest { origin, records } => {
                    assert_eq!(origin, remote_origin);
                    assert_eq!(records.len(), 1);
                    assert_eq!(records[0].source_id, 129);
                    assert_eq!(records[0].raw.as_bytes(), raw(129));
                }
                other => panic!("unexpected later forward: {other:?}"),
            }
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            // Close without a receipt: the later request's outcome is unknown.
            drop(stream);
        });
        // Exactly one tick starts the later RPC. Await its actual request without
        // ticking, so confirmed rows demonstrably remain when the worker blocks.
        daemon.tick().unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(daemon.store.reclaimed().unwrap(), 64);
        assert!(daemon.forward_worker.is_some());
        daemon.tick().unwrap();
        assert_eq!(daemon.store.reclaimed().unwrap(), 96);
        daemon.tick().unwrap();
        assert_eq!(daemon.store.reclaimed().unwrap(), 128);
        assert_eq!(daemon.store.forwarded().unwrap(), 128);
        assert_eq!(daemon.store.count().unwrap(), 1);
        assert_eq!(daemon.store.read_raw(128, 1).unwrap()[0], (129, raw(129)));
        assert!(matches!(
            daemon.forward_worker.as_ref().unwrap().try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        let path = config.socket.clone();
        let admission = std::thread::spawn(move || {
            super::super::transport::submit_bytes(
                &path,
                &raw(130),
                Instant::now() + Duration::from_secs(1),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while !admission.is_finished() {
            daemon.tick().unwrap();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(admission.join().unwrap().unwrap().id, 130);
        assert_eq!(daemon.store.count().unwrap(), 2);
        assert_eq!(daemon.store.reclaimed().unwrap(), 128);
        assert_eq!(daemon.store.forwarded().unwrap(), 128);
        assert_eq!(daemon.store.read_raw(129, 1).unwrap()[0], (130, raw(130)));
        assert_eq!(daemon.health.errors, 0);
        assert_eq!(daemon.health.refusals, 0);
        release_tx.send(()).unwrap();
        remote.join().unwrap();
        assert!(
            daemon
                .forward_worker
                .take()
                .unwrap()
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .is_err()
        );
        drop(daemon);
        let store = Store::open(&config.storage, config.quota_bytes).unwrap();
        assert_eq!(store.highest_record_id().unwrap(), 130);
        assert_eq!(store.forwarded().unwrap(), 128);
        assert_eq!(store.reclaimed().unwrap(), 128);
        assert_eq!(store.count().unwrap(), 2);
        assert_eq!(store.read_raw(128, 1).unwrap()[0], (129, raw(129)));
        assert_eq!(store.read_raw(129, 1).unwrap()[0], (130, raw(130)));
    }
}
