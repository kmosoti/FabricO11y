//! Bounded admission, one background delivery worker, and deadline custody flush.
use crate::observation::{self, Data, Identity, Observation};
use crate::spindle::custody::Receipt;
use crate::spindle::{Reply, Request, transport};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    io::Cursor,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, TryLockError},
    thread,
    time::{Duration, Instant},
};

mod batch;
pub use batch::{BatchClient, BatchError, BatchReport, BatchStatus, batch_adapter_reply};

#[derive(Clone, Debug)]
pub struct Config {
    pub socket: PathBuf,
    pub producer: String,
    pub generation: String,
    pub resource: String,
    pub slots: usize,
    pub max_record: usize,
    pub delivery_timeout: Duration,
}
impl Config {
    pub fn local(socket: PathBuf, producer: String) -> Result<Self, String> {
        Ok(Self {
            socket,
            producer,
            generation: std::fs::read_to_string("/proc/sys/kernel/random/uuid")
                .map_err(|e| e.to_string())?
                .trim()
                .to_owned(),
            resource: "local".into(),
            slots: 64,
            max_record: observation::MAX_FRAME,
            delivery_timeout: Duration::from_millis(500),
        })
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Admission {
    Accepted { sequence: u64 },
    Invalid,
    TooLarge,
    Full,
    Contended,
    Closed,
    Exhausted,
    Expired,
}
#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub accepted: u64,
    pub confirmed: u64,
    pub charged_slots: usize,
    pub retries: u64,
    pub last_error: Option<String>,
    pub reserved_payload_bytes: usize,
}
struct Buffer {
    bytes: Box<[u8]>,
    credit: admission_kernel::Slot,
    sequence: u64,
}
struct State {
    free: Vec<Buffer>,
    ready: VecDeque<Buffer>,
    accepted: u64,
    confirmed: u64,
    last_receipt: Option<Receipt>,
    inflight: Option<usize>,
    closed: bool,
    retries: u64,
    last_error: Option<String>,
}
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    config: Config,
    context: Option<crate::run::RunContext>,
    expires: Option<u64>,
}
/// Dropping closes admission; the detached single worker exits after its bounded I/O.
/// Pending observations are memory-only until a successful flush. Use flush first.
pub struct Client {
    shared: Arc<Shared>,
}
impl Client {
    pub fn new(config: Config) -> Result<Self, String> {
        Self::with_context(config, None, None)
    }
    fn with_context(
        config: Config,
        context: Option<crate::run::RunContext>,
        expires: Option<u64>,
    ) -> Result<Self, String> {
        if observation::reserved_producer(&config.producer) {
            return Err("collector producer namespace is reserved".into());
        }
        if config.slots == 0
            || config.slots > 4096
            || config.max_record < 256
            || config.max_record > observation::MAX_FRAME
            || config.delivery_timeout.is_zero()
            || config.delivery_timeout > Duration::from_secs(5)
        {
            return Err("invalid SDK limits".into());
        }
        let probe = Observation {
            version: 2,
            registry: observation::REGISTRY.into(),
            identity: Identity {
                producer: config.producer.clone(),
                generation: config.generation.clone(),
                sequence: 1,
            },
            timestamp_ns: 0,
            resource: config.resource.clone(),
            data: Data::Event {
                name: "probe".into(),
                message: "probe".into(),
            },
        };
        probe.validate()?;
        let free = Vec::from_fn(config.slots, |id| Buffer {
            bytes: vec![0; config.max_record].into_boxed_slice(),
            credit: admission_kernel::Slot::new(id, config.max_record),
            sequence: 0,
        });
        let ready = VecDeque::with_capacity(config.slots);
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                free,
                ready,
                accepted: 0,
                confirmed: 0,
                last_receipt: None,
                inflight: None,
                closed: false,
                retries: 0,
                last_error: None,
            }),
            changed: Condvar::new(),
            config,
            context,
            expires,
        });
        let worker = shared.clone();
        thread::Builder::new()
            .name("fabric-delivery".into())
            .spawn(move || deliver(worker))
            .map_err(|e| e.to_string())?;
        Ok(Self { shared })
    }
    /// Caller owns the input. Encoding writes only into a preallocated charged slot.
    pub fn try_record(&self, data: Data, timestamp_ns: u64) -> Admission {
        let mut state = match self.shared.state.try_lock() {
            Ok(s) => s,
            Err(TryLockError::WouldBlock) => return Admission::Contended,
            Err(_) => return Admission::Closed,
        };
        if state.closed {
            return Admission::Closed;
        }
        if self.shared.expires.is_some_and(|expires| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(true, |now| now.as_secs() >= expires)
        }) {
            return Admission::Expired;
        }
        if state.accepted == u64::MAX {
            return Admission::Exhausted;
        }
        let Some(sequence) = admission_kernel::delivery::next_sequence(
            state.accepted,
            state.confirmed,
            self.shared.config.slots as u64,
        ) else {
            return Admission::Full;
        };
        let Some(mut buffer) = state.free.pop() else {
            return Admission::Full;
        };
        assert!(
            buffer.credit.reserve(),
            "free buffer must own a free credit"
        );
        let valid = self.shared.context.as_ref().map_or_else(
            || data.validate(),
            |context| data.validate_in_context(context),
        );
        if !valid {
            assert!(buffer.credit.cancel());
            state.free.push(buffer);
            return Admission::Invalid;
        }
        #[derive(Serialize)]
        struct BorrowedIdentity<'a> {
            producer: &'a str,
            generation: &'a str,
            sequence: u64,
        }
        #[derive(Serialize)]
        struct BorrowedObservation<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            context: Option<&'a crate::run::RunContext>,
            version: u32,
            registry: &'a str,
            identity: BorrowedIdentity<'a>,
            timestamp_ns: u64,
            resource: &'a str,
            data: &'a Data,
        }
        let observation = BorrowedObservation {
            context: self.shared.context.as_ref(),
            version: if self.shared.context.is_some() { 3 } else { 2 },
            registry: if self.shared.context.is_some() {
                observation::REGISTRY_V3
            } else {
                observation::REGISTRY
            },
            identity: BorrowedIdentity {
                producer: &self.shared.config.producer,
                generation: &self.shared.config.generation,
                sequence,
            },
            timestamp_ns,
            resource: &self.shared.config.resource,
            data: &data,
        };
        let mut cursor = Cursor::new(buffer.bytes.as_mut());
        if serde_json::to_writer(&mut cursor, &observation).is_err() {
            assert!(buffer.credit.cancel());
            state.free.push(buffer);
            return Admission::TooLarge;
        }
        let len = cursor.position() as usize;
        if observation::check_json(&buffer.bytes[..len], observation::MAX_FRAME).is_err() {
            assert!(buffer.credit.cancel());
            state.free.push(buffer);
            return Admission::Invalid;
        }
        assert!(buffer.credit.publish(len));
        buffer.sequence = sequence;
        state.accepted = sequence;
        state.ready.push_back(buffer);
        self.shared.changed.notify_all();
        Admission::Accepted { sequence }
    }
    /// Covers the accepted frontier captured on entry, even if other producers continue.
    pub fn flush(&self, deadline: Instant) -> Result<Option<Receipt>, String> {
        let mut state = self.shared.state.lock().map_err(|_| "SDK poisoned")?;
        let target = state.accepted;
        while !admission_kernel::delivery::covered(state.confirmed, target) {
            let Some(wait) = deadline.checked_duration_since(Instant::now()) else {
                return Err(format!(
                    "flush timeout; delivery outcome unknown: {:?}",
                    state.last_error
                ));
            };
            let (s, result) = self
                .shared
                .changed
                .wait_timeout(state, wait)
                .map_err(|_| "SDK poisoned")?;
            state = s;
            if result.timed_out() && state.confirmed < target {
                return Err("flush timeout; delivery outcome unknown".into());
            }
            if state.closed {
                return Err("SDK closed; delivery outcome unknown".into());
            }
        }
        Ok(state.last_receipt.clone())
    }
    pub fn status(&self) -> Status {
        let s = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        Status {
            accepted: s.accepted,
            confirmed: s.confirmed,
            charged_slots: s.ready.len() + usize::from(s.inflight.is_some()),
            retries: s.retries,
            last_error: s.last_error.clone(),
            reserved_payload_bytes: self.shared.config.slots * self.shared.config.max_record,
        }
    }
    pub fn close(&self) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.closed = true;
            self.shared.changed.notify_all()
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.close()
    }
}
fn deliver(shared: Arc<Shared>) {
    loop {
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        while state.ready.is_empty() && !state.closed {
            state = shared
                .changed
                .wait(state)
                .unwrap_or_else(|e| e.into_inner())
        }
        if state.closed {
            return;
        }
        let mut buffer = state.ready.pop_front().expect("ready");
        let len = buffer
            .credit
            .lease()
            .expect("ready buffer must own a ready credit");
        state.inflight = Some(buffer.credit.id());
        drop(state);
        let result = transport::submit_bytes(
            &shared.config.socket,
            &buffer.bytes[..len],
            Instant::now() + shared.config.delivery_timeout,
        );
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.inflight = None;
        match result {
            Ok(receipt)
                if receipt.identity.producer == shared.config.producer
                    && receipt.identity.generation == shared.config.generation
                    && receipt.identity.sequence == buffer.sequence
                    && admission_kernel::delivery::confirm_sequence(
                        state.accepted,
                        state.confirmed,
                        receipt.identity.sequence,
                    )
                    .is_some() =>
            {
                state.confirmed = buffer.sequence;
                state.last_receipt = Some(receipt);
                state.last_error = None;
                assert!(buffer.credit.release());
                state.free.push(buffer);
                shared.changed.notify_all();
            }
            result => {
                state.last_error = Some(match result {
                    Ok(_) => "mismatched custody identity".into(),
                    Err(e) => e,
                });
                state.retries = state.retries.saturating_add(1);
                assert!(buffer.credit.retry());
                state.ready.push_front(buffer);
                // New admissions notify the worker but must not accelerate a retry.
                let _ = shared
                    .changed
                    .wait_timeout_while(state, Duration::from_millis(100), |s| !s.closed);
            }
        }
    }
}
/// JSONL adapter commands share the SDK instance across input lines.
#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdapterCommand {
    Record { data: Data, timestamp_ns: u64 },
    Flush { timeout_ms: u64 },
    Status,
}
pub fn adapter_reply(client: &Client, bytes: &[u8]) -> serde_json::Value {
    let command = observation::check_json(bytes, observation::MAX_FRAME)
        .and_then(|()| serde_json::from_slice::<AdapterCommand>(bytes).map_err(|e| e.to_string()));
    match command {
        Ok(AdapterCommand::Record { data, timestamp_ns }) => {
            serde_json::json!({"admission":client.try_record(data,timestamp_ns)})
        }
        Ok(AdapterCommand::Flush { timeout_ms }) => {
            match client.flush(Instant::now() + Duration::from_millis(timeout_ms.min(30000))) {
                Ok(r) => serde_json::json!({"receipt":r}),
                Err(e) => serde_json::json!({"error":e}),
            }
        }
        Ok(AdapterCommand::Status) => serde_json::json!({"status":client.status()}),
        Err(e) => serde_json::json!({"error":e}),
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use std::{
        collections::BTreeSet,
        io::{Read, Write},
        os::unix::net::UnixListener,
        sync::mpsc,
    };

    fn assert_pool(client: &Client) {
        let state = client.shared.state.lock().unwrap();
        let mut ids = BTreeSet::new();
        for buffer in state.free.iter().chain(state.ready.iter()) {
            assert!(ids.insert(buffer.credit.id()), "duplicated credit");
            assert_eq!(buffer.credit.capacity(), buffer.bytes.len());
            assert!(buffer.credit.encoded_len() <= buffer.bytes.len());
        }
        if let Some(id) = state.inflight {
            assert!(ids.insert(id));
        }
        assert_eq!(ids, (0..client.shared.config.slots).collect());
        assert_eq!(
            state.accepted - state.confirmed,
            (state.ready.len() + usize::from(state.inflight.is_some())) as u64
        );
        for buffer in &state.free {
            assert_eq!(buffer.credit.encoded_len(), 0);
        }
        for (offset, buffer) in state.ready.iter().enumerate() {
            assert_eq!(
                buffer.sequence,
                state.confirmed + 1 + u64::from(state.inflight.is_some()) + offset as u64
            );
        }
    }

    fn admit(client: &Client, data: Data) -> Admission {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let result = client.try_record(data.clone(), 1);
            if result != Admission::Contended {
                return result;
            }
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
    }

    #[test]
    fn mismatched_receipts_preserve_credit_and_retry_exact_payload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("socket");
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let (advance, proceed) = mpsc::channel();
        let peer = thread::spawn(move || {
            let mut original = None;
            for attempt in 0..4 {
                proceed.recv_timeout(Duration::from_secs(5)).unwrap();
                let deadline = Instant::now() + Duration::from_secs(3);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline);
                            thread::sleep(Duration::from_millis(1));
                        }
                        Err(e) => panic!("{e}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut header = [0; 4];
                stream.read_exact(&mut header).unwrap();
                let len = u32::from_be_bytes(header) as usize;
                assert!(len <= observation::MAX_FRAME);
                let mut bytes = vec![0; len];
                stream.read_exact(&mut bytes).unwrap();
                if let Some(original) = &original {
                    assert_eq!(&bytes, original);
                } else {
                    original = Some(bytes.clone());
                }
                let observation: Observation = serde_json::from_slice(&bytes).unwrap();
                let mut identity = observation.identity;
                match attempt {
                    0 => identity.producer.push_str("-wrong"),
                    1 => identity.generation.push_str("-wrong"),
                    2 => identity.sequence += 1,
                    _ => {}
                }
                let reply = serde_json::to_vec(&Reply::Receipt {
                    receipt: Receipt {
                        id: 1,
                        identity,
                        duplicate: attempt > 0,
                    },
                })
                .unwrap();
                stream
                    .write_all(&(reply.len() as u32).to_be_bytes())
                    .unwrap();
                stream.write_all(&reply).unwrap();
            }
        });
        let mut config = Config::local(path, "credit-test".into()).unwrap();
        config.slots = 1;
        config.delivery_timeout = Duration::from_secs(3);
        let client = Client::new(config).unwrap();
        assert_eq!(
            admit(
                &client,
                Data::Event {
                    name: "test".into(),
                    message: "same bytes".into()
                }
            ),
            Admission::Accepted { sequence: 1 }
        );
        for expected_retries in 1..=3 {
            advance.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            while client.status().retries < expected_retries {
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(1));
            }
            assert_pool(&client);
            let status = client.status();
            assert_eq!(status.confirmed, 0);
            assert_eq!(status.charged_slots, 1);
            assert_eq!(
                admit(
                    &client,
                    Data::Event {
                        name: "full".into(),
                        message: "payload".into()
                    }
                ),
                Admission::Full
            );
        }
        advance.send(()).unwrap();
        client
            .flush(Instant::now() + Duration::from_secs(3))
            .unwrap();
        assert_pool(&client);
        assert_eq!(client.status().charged_slots, 0);
        peer.join().unwrap();
    }

    #[test]
    fn rejected_encoding_and_concurrent_outage_preserve_pool() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::local(dir.path().join("absent"), "pool-test".into()).unwrap();
        config.slots = 8;
        config.max_record = 512;
        config.delivery_timeout = Duration::from_millis(50);
        let client = Arc::new(Client::new(config).unwrap());
        assert_eq!(
            admit(
                &client,
                Data::Log {
                    level: observation::Level::Info,
                    message: "x".repeat(1024)
                }
            ),
            Admission::TooLarge
        );
        assert_eq!(
            admit(
                &client,
                Data::Metric {
                    name: "".into(),
                    value: f64::NAN,
                    metric_type: observation::MetricType::Gauge,
                    reset: false,
                    unit: "".into()
                }
            ),
            Admission::Invalid
        );
        assert_pool(&client);
        assert_eq!(client.status().accepted, 0);
        thread::scope(|scope| {
            for _ in 0..4 {
                let client = client.clone();
                scope.spawn(move || {
                    for _ in 0..16 {
                        assert!(matches!(
                            admit(
                                &client,
                                Data::Event {
                                    name: "parallel".into(),
                                    message: "payload".into()
                                }
                            ),
                            Admission::Accepted { .. } | Admission::Full
                        ));
                        assert_pool(&client);
                    }
                });
            }
        });
        assert_pool(&client);
        assert_eq!(client.status().accepted, 8);
        assert_eq!(client.status().charged_slots, 8);
        client.close();
    }
}

/// Context-aware v3 producer. Construction and renewal perform explicit control I/O;
/// recording retains the bounded, nonblocking admission path.
pub struct RunClient {
    client: Client,
    context: crate::run::RunContext,
    config: Config,
    origin: String,
    authority: Option<String>,
    fabric_target: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FabricReceipt {
    pub authority: String,
    pub origin: String,
    pub through: u64,
}
fn validate_context_config(config: &Config) -> Result<(), String> {
    if config.slots == 0
        || config.slots > 4096
        || config.max_record < 256
        || config.max_record > observation::MAX_FRAME
        || config.delivery_timeout.is_zero()
        || config.delivery_timeout > Duration::from_secs(5)
        || observation::reserved_producer(&config.producer)
    {
        return Err("invalid context client configuration".into());
    }
    Observation {
        version: 2,
        registry: observation::REGISTRY.into(),
        identity: Identity {
            producer: config.producer.clone(),
            generation: "validation".into(),
            sequence: 1,
        },
        timestamp_ns: 0,
        resource: config.resource.clone(),
        data: Data::Event {
            name: "probe".into(),
            message: "probe".into(),
        },
    }
    .validate()
}

#[derive(Debug)]
enum LeaseError {
    Capacity { used: u64, limit: u64 },
    Other(String),
}

fn request_lease(
    config: &Config,
    expected_origin: Option<&str>,
    deadline: Instant,
) -> Result<(String, u64), LeaseError> {
    let producer = config.producer.clone();
    let request = match expected_origin {
        Some(origin) => Request::LeaseForOrigin {
            producer,
            expected_origin: origin.to_owned(),
        },
        None => Request::Lease { producer },
    };
    match transport::request(&config.socket, &request, deadline).map_err(LeaseError::Other)? {
        Reply::Lease {
            generation,
            expires_unix_s,
        } => Ok((generation, expires_unix_s)),
        Reply::GenerationCapacity { used, limit } => Err(LeaseError::Capacity { used, limit }),
        Reply::Error { error } => Err(LeaseError::Other(error)),
        _ => Err(LeaseError::Other("unexpected lease response".into())),
    }
}

impl RunClient {
    pub fn new(mut config: Config, context: crate::run::RunContext) -> Result<Self, String> {
        context.validate()?;
        validate_context_config(&config)?;
        let (generation, expires) =
            request_lease(&config, None, Instant::now() + config.delivery_timeout).map_err(
                |error| match error {
                    LeaseError::Capacity { .. } => "active generation limit".to_owned(),
                    LeaseError::Other(error) => error,
                },
            )?;
        let (authority, origin) = match transport::request(
            &config.socket,
            &Request::FabricProgress,
            Instant::now() + config.delivery_timeout,
        )? {
            Reply::FabricProgress {
                authority, origin, ..
            } => (authority, origin),
            Reply::Error { error } => return Err(error),
            _ => return Err("unexpected progress response".into()),
        };
        config.generation = generation;
        let client = Client::with_context(config.clone(), Some(context.clone()), Some(expires))?;
        Ok(Self {
            client,
            context,
            config,
            origin,
            authority,
            fabric_target: None,
        })
    }
    pub fn context(&self) -> &crate::run::RunContext {
        &self.context
    }
    pub fn try_record(&self, data: Data, timestamp_ns: u64) -> Admission {
        self.client.try_record(data, timestamp_ns)
    }
    pub fn status(&self) -> Status {
        self.client.status()
    }
    pub fn flush_local(&self, deadline: Instant) -> Result<Option<Receipt>, String> {
        self.client.flush(deadline)
    }
    pub fn flush_to_fabric(&self, deadline: Instant) -> Result<Option<FabricReceipt>, String> {
        let receipt = self.flush_local(deadline)?;
        let target = match (receipt.as_ref().map(|r| r.id), self.fabric_target) {
            (Some(a), Some(b)) => a.max(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => return Ok(None),
        };
        let expected = self
            .authority
            .as_ref()
            .ok_or("canonical forwarding is not configured")?;
        loop {
            match transport::request(&self.config.socket, &Request::FabricProgress, deadline)? {
                Reply::FabricProgress {
                    authority: Some(authority),
                    origin,
                    through,
                } if &authority == expected && origin == self.origin => {
                    if through >= target {
                        return Ok(Some(FabricReceipt {
                            authority,
                            origin,
                            through,
                        }));
                    }
                }
                Reply::Error { error } => return Err(error),
                _ => return Err("canonical authority or origin changed".into()),
            }
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or("Fabric flush deadline; outcome unknown")?;
            thread::sleep(remaining.min(Duration::from_millis(10)));
        }
    }
    /// Requires all accepted records to have local custody before replacing the lease.
    pub fn renew(&mut self) -> Result<(), String> {
        let status = self.client.status();
        if status.accepted != status.confirmed || status.charged_slots != 0 {
            return Err("renewal requires a quiescent client".into());
        }
        let previous = self.client.flush(Instant::now())?.map(|receipt| receipt.id);
        let mut renewed = Self::new(self.config.clone(), self.context.clone())?;
        renewed.fabric_target = match (self.fabric_target, previous) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, None) | (None, a) => a,
        };
        *self = renewed;
        Ok(())
    }
    pub fn close(&self) {
        self.client.close();
    }
}

pub fn run_adapter_reply(client: &mut RunClient, bytes: &[u8]) -> serde_json::Value {
    #[derive(Deserialize)]
    #[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
    enum Command {
        Record { data: Data, timestamp_ns: u64 },
        Flush { timeout_ms: u64 },
        FlushToFabric { timeout_ms: u64 },
        Status,
        Context,
        Renew,
    }
    match observation::check_json(bytes, observation::MAX_FRAME)
        .and_then(|()| serde_json::from_slice::<Command>(bytes).map_err(|e| e.to_string()))
    {
        Ok(Command::Record { data, timestamp_ns }) => {
            serde_json::json!({"admission":client.try_record(data,timestamp_ns)})
        }
        Ok(Command::Flush { timeout_ms }) => match client
            .flush_local(Instant::now() + Duration::from_millis(timeout_ms.min(30000)))
        {
            Ok(receipt) => serde_json::json!({"receipt":receipt}),
            Err(error) => serde_json::json!({"error":error}),
        },
        Ok(Command::FlushToFabric { timeout_ms }) => match client
            .flush_to_fabric(Instant::now() + Duration::from_millis(timeout_ms.min(30000)))
        {
            Ok(receipt) => serde_json::json!({"fabric_receipt":receipt}),
            Err(error) => serde_json::json!({"error":error}),
        },
        Ok(Command::Status) => serde_json::json!({"status":client.status()}),
        Ok(Command::Context) => serde_json::json!({"context":client.context()}),
        Ok(Command::Renew) => match client.renew() {
            Ok(()) => serde_json::json!({"renewed":true}),
            Err(error) => serde_json::json!({"error":error}),
        },
        Err(error) => serde_json::json!({"error":error}),
    }
}

#[cfg(test)]
mod context_admission_tests {
    use super::*;
    #[test]
    fn mismatched_run_identity_is_refused_before_delivery() {
        let temp = tempfile::tempdir().unwrap();
        let config = Config::local(temp.path().join("absent"), "context".into()).unwrap();
        let context = crate::run::RunContext::new().unwrap();
        let client = Client::with_context(config, Some(context.clone()), Some(u64::MAX)).unwrap();
        for tool in [false, true] {
            let data = if tool {
                Data::ToolExecution {
                    tool: "fixture".into(),
                    run_id: "local-label".into(),
                    duration_ns: 1,
                    status: observation::Outcome::Ok,
                }
            } else {
                Data::AgentRun {
                    agent: "fixture".into(),
                    run_id: "local-label".into(),
                    duration_ns: 1,
                    status: observation::Outcome::Ok,
                }
            };
            loop {
                let result = client.try_record(data.clone(), 1);
                if result == Admission::Contended {
                    continue;
                }
                assert_eq!(result, Admission::Invalid);
                break;
            }
        }
        assert_eq!(client.status().accepted, 0);
    }
}
