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
    len: usize,
    sequence: u64,
}
struct State {
    free: Vec<Buffer>,
    ready: VecDeque<Buffer>,
    accepted: u64,
    confirmed: u64,
    last_receipt: Option<Receipt>,
    inflight: bool,
    closed: bool,
    retries: u64,
    last_error: Option<String>,
}
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    config: Config,
}
/// Dropping closes admission; the detached single worker exits after its bounded I/O.
/// Pending observations are memory-only until a successful flush. Use flush first.
pub struct Client {
    shared: Arc<Shared>,
}
impl Client {
    pub fn new(config: Config) -> Result<Self, String> {
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
        let free = (0..config.slots)
            .map(|_| Buffer {
                bytes: vec![0; config.max_record].into_boxed_slice(),
                len: 0,
                sequence: 0,
            })
            .collect();
        let ready = VecDeque::with_capacity(config.slots);
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                free,
                ready,
                accepted: 0,
                confirmed: 0,
                last_receipt: None,
                inflight: false,
                closed: false,
                retries: 0,
                last_error: None,
            }),
            changed: Condvar::new(),
            config,
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
        let Some(sequence) = state.accepted.checked_add(1) else {
            return Admission::Exhausted;
        };
        let Some(mut buffer) = state.free.pop() else {
            return Admission::Full;
        };
        if !data.validate() {
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
            version: u32,
            registry: &'a str,
            identity: BorrowedIdentity<'a>,
            timestamp_ns: u64,
            resource: &'a str,
            data: &'a Data,
        }
        let observation = BorrowedObservation {
            version: 2,
            registry: observation::REGISTRY,
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
            state.free.push(buffer);
            return Admission::TooLarge;
        }
        buffer.len = cursor.position() as usize;
        if observation::check_json(&buffer.bytes[..buffer.len], observation::MAX_FRAME).is_err() {
            state.free.push(buffer);
            return Admission::Invalid;
        }
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
        while state.confirmed < target {
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
            charged_slots: s.ready.len() + usize::from(s.inflight),
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
        let buffer = state.ready.pop_front().expect("ready");
        state.inflight = true;
        drop(state);
        let result = transport::submit_bytes(
            &shared.config.socket,
            &buffer.bytes[..buffer.len],
            Instant::now() + shared.config.delivery_timeout,
        );
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.inflight = false;
        match result {
            Ok(receipt)
                if receipt.identity.producer == shared.config.producer
                    && receipt.identity.generation == shared.config.generation
                    && receipt.identity.sequence == buffer.sequence =>
            {
                state.confirmed = buffer.sequence;
                state.last_receipt = Some(receipt);
                state.last_error = None;
                state.free.push(buffer);
                shared.changed.notify_all();
            }
            result => {
                state.last_error = Some(match result {
                    Ok(_) => "mismatched custody identity".into(),
                    Err(e) => e,
                });
                state.retries = state.retries.saturating_add(1);
                state.ready.push_front(buffer);
                let _ = shared
                    .changed
                    .wait_timeout(state, Duration::from_millis(100));
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
// Keep transport command types part of the public SDK protocol surface.
pub fn remote_status(socket: &std::path::Path) -> Result<Reply, String> {
    transport::request(
        socket,
        &Request::Status,
        Instant::now() + Duration::from_secs(2),
    )
}
