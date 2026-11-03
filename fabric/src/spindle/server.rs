use super::{
    Config, Health, Reply, Request,
    custody::Store,
    sources::{self, FileSource, HostSource, JournalSource},
};
use crate::observation;
use rustix::net::sockopt::socket_peercred;
use std::{
    fs,
    io::{Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
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
pub struct Daemon {
    config: Config,
    store: Store,
    listener: UnixListener,
    connections: Vec<Connection>,
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
        let connections = Vec::with_capacity(config.max_connections);
        Ok(Self {
            config,
            store,
            listener,
            connections,
            files,
            journal,
            host,
            health: Health::default(),
            start: Instant::now(),
            stopping: false,
            socket_identity,
            next_source: 0,
            forward_worker: None,
            next_forward: Instant::now(),
        })
    }
    pub fn run(&mut self) -> Result<(), String> {
        while !self.stopping || !self.connections.is_empty() {
            let ingestion_before = self.health.ingestion;
            self.tick()?;
            // Successful admissions have durable replies queued for the next
            // tick. Flush them promptly; remote/output-only waits still yield.
            if self.health.ingestion == ingestion_before {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        Ok(())
    }
    pub fn tick(&mut self) -> Result<(), String> {
        self.forward_tick();
        if !self.stopping {
            for _ in 0..self.config.max_connections {
                match self.listener.accept() {
                    Ok((stream, _)) => {
                        if self.connections.len() >= self.config.max_connections {
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
                            remote: None,
                            deadline: Instant::now()
                                + Duration::from_millis(self.config.client_timeout_ms),
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => return Err(err(e)),
                }
            }
        }
        self.store.begin_connection_batch()?;
        let batch_started = Instant::now();
        let ingestion_before = self.health.ingestion;
        let retries_before = self.health.retries;
        let batch = self
            .progress_connections()
            .and_then(|()| self.store.commit_connection_batch());
        if let Err(error) = batch {
            let rollback = self.store.rollback_connection_batch();
            self.health.ingestion = ingestion_before;
            self.health.retries = retries_before;
            // Never leave rolled-back receipts available to a later tick, even
            // when an embedding caller catches the fatal error from tick().
            self.connections.clear();
            return Err(match rollback {
                Ok(()) => error,
                Err(rollback) => format!("{error}; custody batch rollback failed: {rollback}"),
            });
        }
        if self.health.ingestion != ingestion_before {
            // Group commit latency includes the bounded synchronous batch and
            // its durable outer RELEASE, rather than only an inner savepoint.
            self.health.last_commit_latency_ns =
                batch_started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
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
        }
        Ok(())
    }
    fn progress_connections(&mut self) -> Result<(), String> {
        let mut index = self.connections.len();
        while index > 0 {
            index -= 1;
            let mut connection = self.connections.swap_remove(index);
            let keep = if Instant::now() >= connection.deadline {
                self.health.refusals += 1;
                false
            } else {
                match connection.progress() {
                    Ok(ConnectionProgress::Request(body)) => {
                        match self.prepare_remote(&body) {
                            Ok(Some(job)) => {
                                let (send, receive) = std::sync::mpsc::sync_channel(1);
                                let deadline = connection.deadline;
                                match std::thread::Builder::new()
                                    .name("fabric-resolve".into())
                                    .spawn(move || {
                                        let _ = send.send(job.execute(deadline));
                                    }) {
                                    Ok(_) => {
                                        connection.remote = Some(receive);
                                        self.connections.push(connection);
                                        continue;
                                    }
                                    Err(error) => {
                                        connection.output = serde_json::to_vec(&Reply::Error {
                                            error: error.to_string(),
                                        })
                                        .map_err(err)?
                                    }
                                }
                            }
                            Ok(None) => connection.output = self.handle(&body)?,
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
        self.health.records = self.store.count().unwrap_or(0);
        self.health.highest_record_id = self.store.highest_record_id().unwrap_or(0);
        self.health.custody_bytes = self.store.disk_bytes().unwrap_or(0);
        self.health.active_connections = self.connections.len();
        self.health.queue_bytes = self
            .connections
            .iter()
            .map(|c| c.body.capacity() + c.output.capacity())
            .sum();
        self.health.uptime_ms = self.start.elapsed().as_millis() as u64;
        self.health.process_resident_bytes = sources::process_resident_bytes();
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
    fn prepare_remote(
        &mut self,
        bytes: &[u8],
    ) -> Result<Option<super::custody::RemoteJob>, String> {
        observation::check_json(bytes, observation::MAX_FRAME)?;
        let value: serde_json::Value = serde_json::from_slice(bytes).map_err(err)?;
        if value.get("command").is_some() {
            if let Request::Read { after, limit } = serde_json::from_value(value).map_err(err)? {
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
    fn handle(&mut self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        let request = observation::check_json(bytes, observation::MAX_FRAME)
            .and_then(|()| serde_json::from_slice::<serde_json::Value>(bytes).map_err(err));
        let reply = match request {
            Ok(v) if v.get("command").is_some() => match serde_json::from_value::<Request>(v) {
                Ok(Request::Status) => Reply::Status {
                    status: self.health(),
                },
                Ok(Request::Read { after, limit }) => {
                    match self.store.read_raw(after, limit.min(1)) {
                        Ok(records) => return super::wire::encode_records(&records),
                        Err(error) => Reply::Error { error },
                    }
                }
                Ok(Request::Lease { producer }) => match self.store.issue_lease(&producer) {
                    Ok((generation, expires_unix_s)) => Reply::Lease {
                        generation,
                        expires_unix_s,
                    },
                    Err(error) => Reply::Error { error },
                },
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
                    return serde_json::to_vec(&Reply::Error {
                        error: "collector producer namespace is reserved".into(),
                    })
                    .map_err(err);
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
        serde_json::to_vec(&reply).map_err(err)
    }
    fn forward_tick(&mut self) {
        use std::sync::mpsc::TryRecvError;
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
        if let Err(error) = self.store.reclaim_confirmed() {
            self.health.forwarding_error = Some(error);
            self.next_forward = Instant::now() + Duration::from_millis(100);
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
        match std::thread::Builder::new()
            .name("fabric-forward".into())
            .spawn(move || {
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
            Ok(_) => self.forward_worker = Some(receive),
            Err(error) => self.health.forwarding_error = Some(error.to_string()),
        }
    }
    pub fn socket(&self) -> &std::path::Path {
        &self.config.socket
    }
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
    fn progress(&mut self) -> Result<ConnectionProgress, String> {
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
            return self.write_reply();
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
    fn write_reply(&mut self) -> Result<ConnectionProgress, String> {
        for _ in 0..8 {
            match self.stream.write(&self.output[self.written..]) {
                Ok(0) => return Err("reply write returned zero".into()),
                Ok(n) => {
                    self.written += n;
                    if self.written == self.output.len() {
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
    use crate::canonical::{self, Authenticated, CanonicalRow, Response};
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
                let request: Authenticated =
                    serde_json::from_slice(&canonical::read_frame(&mut stream, deadline).unwrap())
                        .unwrap();
                assert_eq!(request.credential, "fixture-key");
                let response = match request.request {
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
        let read = std::thread::spawn(move || {
            super::super::transport::read_raw(&socket, 0, Instant::now() + Duration::from_secs(2))
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
