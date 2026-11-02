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
        let store = Store::open(&config.storage, config.quota_bytes)?;
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
        })
    }
    pub fn run(&mut self) -> Result<(), String> {
        while !self.stopping || !self.connections.is_empty() {
            self.tick()?;
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }
    pub fn tick(&mut self) -> Result<(), String> {
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
                            deadline: Instant::now()
                                + Duration::from_millis(self.config.client_timeout_ms),
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => return Err(err(e)),
                }
            }
        }
        let mut index = self.connections.len();
        while index > 0 {
            index -= 1;
            let mut connection = self.connections.swap_remove(index);
            let keep = if Instant::now() >= connection.deadline {
                self.health.refusals += 1;
                false
            } else {
                match connection.progress() {
                    Ok(Some(body)) => {
                        let reply = self.handle(&body);
                        connection.output = serde_json::to_vec(&reply).map_err(err)?;
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
                        connection.write_reply()
                    }
                    Ok(None) => true,
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
    fn health(&mut self) -> Health {
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
    fn handle(&mut self, bytes: &[u8]) -> Reply {
        let request = observation::check_json(bytes, observation::MAX_FRAME)
            .and_then(|()| serde_json::from_slice::<serde_json::Value>(bytes).map_err(err));
        match request {
            Ok(v) if v.get("command").is_some() => match serde_json::from_value::<Request>(v) {
                Ok(Request::Status) => Reply::Status {
                    status: self.health(),
                },
                Ok(Request::Read { after, limit }) => match self.store.read(after, limit.min(1)) {
                    Ok(records) => Reply::Records { records },
                    Err(error) => Reply::Error { error },
                },
                Ok(Request::Stop) => {
                    self.stopping = true;
                    Reply::Stopped
                }
                Err(e) => Reply::Error { error: err(e) },
            },
            Ok(_) => {
                if observation::decode(bytes)
                    .is_ok_and(|o| observation::reserved_producer(&o.identity.producer))
                {
                    self.health.refusals += 1;
                    return Reply::Error {
                        error: "collector producer namespace is reserved".into(),
                    };
                }
                let start = Instant::now();
                let result = self.store.submit(bytes, None);
                self.health.last_commit_latency_ns =
                    start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
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
    fn progress(&mut self) -> Result<Option<Vec<u8>>, String> {
        if !self.output.is_empty() {
            return if self.write_reply() {
                Ok(None)
            } else {
                Err("reply complete".into())
            };
        }
        while self.header_used < 4 {
            match self.stream.read(&mut self.header[self.header_used..]) {
                Ok(0) => return Err("closed".into()),
                Ok(n) => self.header_used += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
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
                return Ok(Some(std::mem::take(&mut self.body)));
            }
            let len = (size - self.body.len()).min(chunk.len());
            match self.stream.read(&mut chunk[..len]) {
                Ok(0) => return Err("closed".into()),
                Ok(n) => self.body.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
                Err(e) => return Err(err(e)),
            }
        }
        if self.body.len() == size {
            Ok(Some(std::mem::take(&mut self.body)))
        } else {
            Ok(None)
        }
    }
    fn write_reply(&mut self) -> bool {
        for _ in 0..8 {
            match self.stream.write(&self.output[self.written..]) {
                Ok(0) => return false,
                Ok(n) => {
                    self.written += n;
                    if self.written == self.output.len() {
                        return false;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return true,
                Err(_) => return false,
            }
        }
        true
    }
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
