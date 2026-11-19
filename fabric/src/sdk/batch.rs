//! Durable terminal reporting for sequential runs sharing an authenticated lease.
mod outbox;

use super::{Config, request_lease, validate_context_config};
use crate::{
    observation::{self, Data, Identity, ObservationV3},
    run::{RunContext, validate_id},
    spindle::{ORIGIN_MISMATCH, custody::Receipt, transport},
};
use outbox::{Lease, Outbox, Pending, SavedReport};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    path::Path,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// A caller-owned idempotency key and one typed terminal observation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchReport {
    pub report_id: String,
    pub context: RunContext,
    pub data: Data,
    pub timestamp_ns: u64,
}
impl BatchReport {
    fn digest(&self) -> Result<[u8; 32], BatchError> {
        validate_id(&self.report_id).map_err(|()| {
            BatchError::Invalid("report_id must be a lowercase nonzero UUID".into())
        })?;
        self.context.validate().map_err(BatchError::Invalid)?;
        if !matches!(
            self.data,
            Data::AgentRun { .. } | Data::ToolExecution { .. }
        ) || !self.data.validate_in_context(&self.context)
        {
            return Err(BatchError::Invalid(
                "terminal data must be an agent_run or tool_execution matching context.run_id"
                    .into(),
            ));
        }
        // Hash the versioned typed request, so JSON whitespace and member ordering do
        // not change its identity. The separate payload is the original v3 wire frame.
        let mut hash = Sha256::new();
        hash.update(b"FabricO11y batch report identity v1\0");
        hash.update(
            serde_json::to_vec(self).map_err(|error| BatchError::Invalid(error.to_string()))?,
        );
        Ok(hash.finalize().into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BatchError {
    Invalid(String),
    Conflict,
    Full,
    Expired,
    OriginMismatch,
    MismatchedReceipt,
    Unavailable(String),
    Storage(String),
}
impl BatchError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid",
            Self::Conflict => "conflict",
            Self::Full => "full",
            Self::Expired => "expired",
            Self::OriginMismatch => "origin_mismatch",
            Self::MismatchedReceipt => "mismatched_receipt",
            Self::Unavailable(_) => "unavailable",
            Self::Storage(_) => "storage",
        }
    }
}
impl fmt::Display for BatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) | Self::Unavailable(message) | Self::Storage(message) => {
                formatter.write_str(message)
            }
            Self::Conflict => {
                formatter.write_str("report_id already belongs to a different request")
            }
            Self::Full => formatter.write_str("outbox has a pending report or reached its limit"),
            Self::Expired => formatter.write_str("generation expired; unresolved report retained"),
            Self::OriginMismatch => formatter.write_str("Spindle origin changed"),
            Self::MismatchedReceipt => formatter.write_str("mismatched custody receipt identity"),
        }
    }
}
impl std::error::Error for BatchError {}

#[derive(Clone, Debug, Serialize)]
pub struct BatchStatus {
    pub origin: Option<String>,
    pub pending_report_id: Option<String>,
    pub expires_unix_s: Option<u64>,
    pub pending_expired: bool,
    pub confirmed: u64,
    pub last_error: Option<String>,
}

/// An exclusively owned SQLite outbox for one producer and Spindle origin.
/// `record` performs synchronous durable I/O. A delivery error can leave a
/// pending report. Retry with the same report ID and fields or call `flush_local`.
pub struct BatchClient {
    config: Config,
    outbox: Outbox,
    last_error: Option<String>,
}
impl BatchClient {
    /// Opens an owner-only directory and attempts a bounded replay of any pending
    /// report. `status` reports an unresolved replay and its last error.
    pub fn open(config: Config, directory: &Path) -> Result<Self, BatchError> {
        validate_context_config(&config).map_err(BatchError::Invalid)?;
        let outbox = Outbox::open(directory, &config.producer, &config.resource)?;
        let mut client = Self {
            config,
            outbox,
            last_error: None,
        };
        if client.outbox.pending_id()?.is_some() {
            let deadline = Instant::now() + client.config.delivery_timeout;
            if let Err(error @ BatchError::Storage(_)) = client.flush_local(deadline) {
                return Err(error);
            }
        }
        Ok(client)
    }

    /// Persists and submits a new report, or returns its previously stored receipt.
    /// The RPC work is limited by `config.delivery_timeout`.
    pub fn record(&mut self, report: BatchReport) -> Result<Receipt, BatchError> {
        let digest = report.digest()?;
        let deadline = Instant::now() + self.config.delivery_timeout;
        if let Some(saved) = self.outbox.find(&report.report_id)? {
            return match saved {
                SavedReport::Confirmed(saved_digest, receipt) if saved_digest == digest => {
                    Ok(receipt)
                }
                SavedReport::Pending(pending) if pending.digest == digest => {
                    self.deliver(&pending, deadline)
                }
                _ => Err(BatchError::Conflict),
            };
        }
        if self.outbox.pending_id()?.is_some() {
            self.check_pending_expiry(&self.outbox.pending_lease()?)?;
            return Err(BatchError::Full);
        }
        self.outbox.check_capacity()?;
        let mut lease = self.lease_for_record(deadline)?;
        lease.sequence += 1;
        let observation = ObservationV3 {
            version: observation::VERSION_V3,
            registry: observation::REGISTRY_V3.into(),
            context: report.context,
            identity: Identity {
                producer: self.config.producer.clone(),
                generation: lease.generation.clone(),
                sequence: lease.sequence,
            },
            timestamp_ns: report.timestamp_ns,
            resource: self.config.resource.clone(),
            data: report.data,
        };
        observation.validate().map_err(BatchError::Invalid)?;
        let bytes = serde_json::to_vec(&observation)
            .map_err(|error| BatchError::Invalid(error.to_string()))?;
        observation::check_json(&bytes, self.config.max_record).map_err(BatchError::Invalid)?;
        let pending = Pending {
            report_id: report.report_id,
            digest,
            bytes,
        };
        self.outbox.stage(&pending, lease.sequence)?;
        self.deliver(&pending, deadline)
    }

    pub fn status(&self) -> Result<BatchStatus, BatchError> {
        let pending_report_id = self.outbox.pending_id()?;
        let lease = self.outbox.lease()?;
        let pending_expired = pending_report_id.is_some()
            && lease.as_ref().is_some_and(|lease| {
                lease.expired || wall_time().is_ok_and(|now| now >= lease.expires)
            });
        Ok(BatchStatus {
            origin: lease.as_ref().map(|lease| lease.origin.clone()),
            pending_report_id,
            expires_unix_s: lease.map(|lease| lease.expires),
            pending_expired,
            confirmed: self.outbox.confirmed_count()?,
            last_error: self.last_error.clone(),
        })
    }

    /// Retries the only pending report up to the deadline, or returns the most
    /// recent confirmed receipt when the outbox is already quiescent.
    pub fn flush_local(&mut self, deadline: Instant) -> Result<Option<Receipt>, BatchError> {
        let Some(pending) = self.outbox.pending()? else {
            return self.outbox.latest_receipt();
        };
        loop {
            let attempt = deadline.min(Instant::now() + self.config.delivery_timeout);
            match self.deliver(&pending, attempt) {
                Ok(receipt) => return Ok(Some(receipt)),
                Err(error @ BatchError::Unavailable(_)) if Instant::now() < deadline => {
                    thread::sleep(
                        deadline
                            .saturating_duration_since(Instant::now())
                            .min(Duration::from_millis(100)),
                    );
                    if Instant::now() >= deadline {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn lease_for_record(&mut self, deadline: Instant) -> Result<Lease, BatchError> {
        let previous = self.outbox.lease()?;
        let now = wall_time()?;
        if let Some(lease) = &previous
            && !lease.expired
            && now < lease.expires
        {
            return Ok(lease.clone());
        }
        let origin = match previous {
            Some(lease) => lease.origin,
            None => transport::read_origin(&self.config.socket, deadline)
                .map_err(BatchError::Unavailable)?,
        };
        if origin.is_empty() || origin.len() > 128 || origin.contains('\0') {
            return Err(BatchError::Unavailable("invalid Spindle origin".into()));
        }
        let (generation, expires) =
            request_lease(&self.config, Some(&origin), deadline).map_err(|error| {
                if error == ORIGIN_MISMATCH {
                    BatchError::OriginMismatch
                } else {
                    BatchError::Unavailable(error)
                }
            })?;
        if generation.is_empty() || generation.len() > 128 || generation.contains('\0') {
            return Err(BatchError::Unavailable("invalid Spindle generation".into()));
        }
        if expires <= wall_time()? || expires > i64::MAX as u64 {
            return Err(BatchError::Unavailable(
                "invalid Spindle lease expiry".into(),
            ));
        }
        let lease = Lease {
            origin,
            generation,
            expires,
            expired: false,
            sequence: 0,
        };
        self.outbox.save_lease(&lease)?;
        Ok(lease)
    }

    fn check_pending_expiry(&self, lease: &Lease) -> Result<(), BatchError> {
        if lease.expired || wall_time()? >= lease.expires {
            if !lease.expired {
                self.outbox.mark_expired()?;
            }
            return Err(BatchError::Expired);
        }
        Ok(())
    }

    fn deliver(&mut self, pending: &Pending, deadline: Instant) -> Result<Receipt, BatchError> {
        let result = self.deliver_once(pending, deadline);
        self.last_error = result.as_ref().err().map(ToString::to_string);
        result
    }

    fn deliver_once(
        &mut self,
        pending: &Pending,
        deadline: Instant,
    ) -> Result<Receipt, BatchError> {
        let lease = self.outbox.pending_lease()?;
        self.check_pending_expiry(&lease)?;
        let observation::StoredObservation::V3(observation) =
            observation::decode_versioned(&pending.bytes).map_err(BatchError::Storage)?
        else {
            return Err(BatchError::Storage("outbox payload is not v3".into()));
        };
        if observation.identity.producer != self.config.producer
            || observation.identity.generation != lease.generation
            || observation.identity.sequence != lease.sequence
            || observation.resource != self.config.resource
            || (BatchReport {
                report_id: pending.report_id.clone(),
                context: observation.context,
                data: observation.data,
                timestamp_ns: observation.timestamp_ns,
            })
            .digest()
            .map_err(|error| BatchError::Storage(error.to_string()))?
                != pending.digest
        {
            return Err(BatchError::Storage(
                "outbox payload identity mismatch".into(),
            ));
        }
        let receipt = match transport::submit_bytes(&self.config.socket, &pending.bytes, deadline) {
            Ok(receipt) => receipt,
            Err(error) if error == "generation expired" => {
                self.outbox.mark_expired()?;
                return Err(BatchError::Expired);
            }
            Err(error) => return Err(BatchError::Unavailable(error)),
        };
        if receipt.id == 0 || receipt.identity != observation.identity {
            return Err(BatchError::MismatchedReceipt);
        }
        self.outbox.confirm(&pending.report_id, &receipt)?;
        Ok(receipt)
    }
}

fn wall_time() -> Result<u64, BatchError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .map_err(|error| BatchError::Unavailable(error.to_string()))
}

pub fn batch_adapter_reply(client: &mut BatchClient, bytes: &[u8]) -> serde_json::Value {
    #[derive(Deserialize)]
    #[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
    enum Command {
        Record(BatchReport),
        Flush { timeout_ms: u64 },
        Status,
    }
    fn error_reply(error: BatchError) -> serde_json::Value {
        serde_json::json!({"error":error.to_string(),"code":error.code()})
    }
    match observation::check_json(bytes, observation::MAX_FRAME)
        .and_then(|()| serde_json::from_slice::<Command>(bytes).map_err(|error| error.to_string()))
    {
        Ok(Command::Record(report)) => {
            let report_id = report.report_id.clone();
            match client.record(report) {
                Ok(receipt) => serde_json::json!({"report_id":report_id,"receipt":receipt}),
                Err(error) => {
                    serde_json::json!({"report_id":report_id,"error":error.to_string(),"code":error.code()})
                }
            }
        }
        Ok(Command::Flush { timeout_ms }) => {
            let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(30000));
            match client.flush_local(deadline) {
                Ok(receipt) => serde_json::json!({"receipt":receipt}),
                Err(error) => error_reply(error),
            }
        }
        Ok(Command::Status) => match client.status() {
            Ok(status) => serde_json::json!({"status":status}),
            Err(error) => error_reply(error),
        },
        Err(error) => error_reply(BatchError::Invalid(error)),
    }
}

#[cfg(test)]
mod tests;
