//! Durable terminal reporting for sequential runs sharing an authenticated lease.
mod outbox;

use super::{Config, LeaseError, request_lease, validate_context_config};
use crate::{
    observation::{self, Data, Identity, ObservationV3},
    run::{RunContext, validate_id},
    spindle::{ORIGIN_MISMATCH, custody::Receipt, transport},
};
use fabric_evidence::{
    ReportScope,
    application_result::{ApplicationPayloadDigest, ApplicationResult},
    evidence_manifest::{OriginalArtifactDigest, ProvenancedTime},
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

/// A terminal application JSON artifact. `artifact_json` is validated and hashed,
/// but its original spelling is not retained in the outbox or custody frame.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationReport {
    pub report_id: String,
    pub context: RunContext,
    pub scope: ReportScope,
    pub parent_report_id: Option<String>,
    pub schema: String,
    pub producer_completion: ProvenancedTime,
    pub artifact_json: String,
    /// Outer observation timestamp, distinct from the producer completion claim.
    pub timestamp_ns: u64,
}

/// Producer claim and a local observation made inside the staging transaction.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApplicationTiming {
    pub producer_completion: ProvenancedTime,
    /// Sampled before the SQLite stage transaction commits; not fsync or Spindle time.
    pub outbox_staged_at: ProvenancedTime,
}

/// State in this private outbox only; absence of timing means unknown.
#[derive(Clone, Debug, Serialize)]
pub struct ReportStatus {
    pub report_id: String,
    pub delivery_state: &'static str,
    pub receipt: Option<Receipt>,
    pub application_timing: Option<ApplicationTiming>,
}

/// A confirmed local custody receipt and its separately named payload digests.
#[derive(Clone, Debug, Serialize)]
pub struct ApplicationReceipt {
    pub receipt: Receipt,
    pub original_artifact_digest: OriginalArtifactDigest,
    pub application_payload_digest: ApplicationPayloadDigest,
    pub application_timing: Option<ApplicationTiming>,
}

const APPLICATION_MARKER: &str = "fabrico11y.application_result.v1";

fn application_digest(
    context: &RunContext,
    timestamp_ns: u64,
    result: &ApplicationResult,
) -> Result<[u8; 32], BatchError> {
    #[derive(Serialize)]
    struct Identity<'a> {
        context: &'a RunContext,
        timestamp_ns: u64,
        result: &'a ApplicationResult,
    }
    let mut hash = Sha256::new();
    hash.update(b"FabricO11y batch application result identity v1\0");
    hash.update(
        serde_json::to_vec(&Identity {
            context,
            timestamp_ns,
            result,
        })
        .map_err(|e| BatchError::Invalid(e.to_string()))?,
    );
    Ok(hash.finalize().into())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BatchError {
    Invalid(String),
    Conflict,
    Full,
    GenerationCapacity { used: u64, limit: u64 },
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
            Self::GenerationCapacity { .. } => "generation_capacity",
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
            Self::GenerationCapacity { used, limit } => write!(
                formatter,
                "Spindle generation capacity exhausted ({used}/{limit}); report not submitted"
            ),
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
        self.outbox.stage(&pending, lease.sequence, None)?;
        self.deliver(&pending, deadline)
    }

    /// Durably stages a validated application result before attempting delivery.
    /// The outbox retains the canonical envelope and source digest, not source bytes.
    pub fn record_application(
        &mut self,
        report: ApplicationReport,
    ) -> Result<ApplicationReceipt, BatchError> {
        report.context.validate().map_err(BatchError::Invalid)?;
        if report.scope.producer() != self.config.producer {
            return Err(BatchError::Invalid(
                "application producer does not match outbox".into(),
            ));
        }
        let result = ApplicationResult::new(
            report.scope,
            report.report_id.clone(),
            report.parent_report_id,
            report.schema,
            report.producer_completion,
            report.artifact_json.as_bytes(),
        )
        .map_err(|e| BatchError::Invalid(format!("invalid application result: {e:?}")))?;
        let digest = application_digest(&report.context, report.timestamp_ns, &result)?;
        let original_artifact_digest = result.original_artifact_digest().clone();
        let application_payload_digest = result.application_payload_digest().clone();
        let result_id = result.report_id().to_owned();
        let receipt = (|| -> Result<Receipt, BatchError> {
            let deadline = Instant::now() + self.config.delivery_timeout;
            let prior_lease = self.outbox.lease()?;
            if let Some(lease) = &prior_lease
                && lease.origin != result.scope().origin()
            {
                return Err(BatchError::OriginMismatch);
            }
            if let Some(saved) = self.outbox.find(result.report_id())? {
                if prior_lease.is_none() {
                    return Err(BatchError::Storage(
                        "saved report has no bound origin".into(),
                    ));
                }
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
            let mut lease =
                self.lease_for_record_for_origin(deadline, Some(result.scope().origin()))?;
            lease.sequence += 1;
            let envelope = result
                .encode_bounded()
                .map_err(|e| BatchError::Invalid(format!("invalid application result: {e:?}")))?;
            let value = serde_json::from_slice(&envelope)
                .map_err(|e| BatchError::Invalid(e.to_string()))?;
            let mut fields = serde_json::Map::new();
            fields.insert(APPLICATION_MARKER.into(), value);
            let observation = ObservationV3 {
                version: observation::VERSION_V3,
                registry: observation::REGISTRY_V3.into(),
                context: report.context,
                identity: Identity {
                    producer: self.config.producer.clone(),
                    generation: lease.generation,
                    sequence: lease.sequence,
                },
                timestamp_ns: report.timestamp_ns,
                resource: self.config.resource.clone(),
                data: Data::JsonLog { fields },
            };
            observation.validate().map_err(BatchError::Invalid)?;
            let bytes =
                serde_json::to_vec(&observation).map_err(|e| BatchError::Invalid(e.to_string()))?;
            observation::check_json(&bytes, self.config.max_record).map_err(BatchError::Invalid)?;
            let pending = Pending {
                report_id: report.report_id,
                digest,
                bytes,
            };
            self.outbox
                .stage(&pending, lease.sequence, Some(result.producer_completion()))?;
            self.deliver(&pending, deadline)
        })()?;
        Ok(ApplicationReceipt {
            receipt,
            original_artifact_digest,
            application_payload_digest,
            application_timing: self.outbox.application_timing(&result_id)?,
        })
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

    /// Returns a single report's owner-outbox state, including unknown legacy timing.
    pub fn report_status(&self, report_id: &str) -> Result<Option<ReportStatus>, BatchError> {
        validate_id(report_id).map_err(|()| {
            BatchError::Invalid("report_id must be a lowercase nonzero UUID".into())
        })?;
        let Some(saved) = self.outbox.find(report_id)? else {
            return Ok(None);
        };
        let (delivery_state, receipt) = match saved {
            SavedReport::Pending(_) => ("outcome_unknown", None),
            SavedReport::Confirmed(_, receipt) => ("custody_confirmed", Some(receipt)),
        };
        Ok(Some(ReportStatus {
            report_id: report_id.into(),
            delivery_state,
            receipt,
            application_timing: self.outbox.application_timing(report_id)?,
        }))
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
        self.lease_for_record_for_origin(deadline, None)
    }

    fn lease_for_record_for_origin(
        &mut self,
        deadline: Instant,
        expected: Option<&str>,
    ) -> Result<Lease, BatchError> {
        let previous = self.outbox.lease()?;
        if let Some(lease) = &previous
            && expected.is_some_and(|origin| origin != lease.origin)
        {
            return Err(BatchError::OriginMismatch);
        }
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
        if expected.is_some_and(|expected| expected != origin) {
            return Err(BatchError::OriginMismatch);
        }
        if origin.is_empty() || origin.len() > 128 || origin.contains('\0') {
            return Err(BatchError::Unavailable("invalid Spindle origin".into()));
        }
        let (generation, expires) =
            request_lease(&self.config, Some(&origin), deadline).map_err(|error| match error {
                LeaseError::Capacity { used, limit } => {
                    BatchError::GenerationCapacity { used, limit }
                }
                LeaseError::Other(error) if error == ORIGIN_MISMATCH => BatchError::OriginMismatch,
                LeaseError::Other(error) => BatchError::Unavailable(error),
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
            || pending_digest(&pending.report_id, &observation, &lease.origin)
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

fn pending_digest(
    report_id: &str,
    observation: &ObservationV3,
    origin: &str,
) -> Result<[u8; 32], BatchError> {
    if let Data::JsonLog { fields } = &observation.data {
        if fields.len() != 1 {
            return Err(BatchError::Invalid("invalid application carrier".into()));
        }
        let value = fields
            .get(APPLICATION_MARKER)
            .ok_or_else(|| BatchError::Invalid("invalid application carrier".into()))?;
        let bytes =
            serde_json::to_vec(value).map_err(|error| BatchError::Invalid(error.to_string()))?;
        let result = ApplicationResult::decode_bounded(&bytes).map_err(|error| {
            BatchError::Invalid(format!("invalid application result: {error:?}"))
        })?;
        if result.report_id() != report_id
            || result.scope().producer() != observation.identity.producer
            || result.scope().origin() != origin
        {
            return Err(BatchError::Invalid(
                "application carrier scope or identity mismatch".into(),
            ));
        }
        application_digest(&observation.context, observation.timestamp_ns, &result)
    } else {
        BatchReport {
            report_id: report_id.to_owned(),
            context: observation.context.clone(),
            data: observation.data.clone(),
            timestamp_ns: observation.timestamp_ns,
        }
        .digest()
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
        RecordApplication(ApplicationReport),
        Flush { timeout_ms: u64 },
        Status,
        ReportStatus { report_id: String },
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
                Ok(receipt) => {
                    serde_json::json!({"report_id":report_id,"delivery_state":"custody_confirmed","projection":"unchecked","receipt":receipt})
                }
                Err(error) => {
                    let state = if matches!(error, BatchError::GenerationCapacity { .. }) {
                        Some("refused_before_acceptance")
                    } else if matches!(
                        error,
                        BatchError::Unavailable(_)
                            | BatchError::MismatchedReceipt
                            | BatchError::Expired
                            | BatchError::Storage(_)
                    ) && client
                        .status()
                        .ok()
                        .and_then(|s| s.pending_report_id)
                        .as_deref()
                        == Some(report_id.as_str())
                    {
                        Some("outcome_unknown")
                    } else {
                        None
                    };
                    let mut result = serde_json::json!({"report_id":report_id,"error":error.to_string(),"code":error.code(),"delivery_state":state});
                    if let BatchError::GenerationCapacity { used, limit } = error {
                        result["capacity"] = serde_json::json!({"used":used,"limit":limit});
                    }
                    result
                }
            }
        }
        Ok(Command::RecordApplication(report)) => {
            let report_id = report.report_id.clone();
            match client.record_application(report) {
                Ok(confirmed) => {
                    serde_json::json!({"report_id":report_id,"delivery_state":"custody_confirmed","durably_staged":true,"projection":"unchecked","receipt":confirmed.receipt,"original_artifact_digest":confirmed.original_artifact_digest,"application_payload_digest":confirmed.application_payload_digest,"application_timing":confirmed.application_timing})
                }
                Err(error) => {
                    let pending = matches!(
                        error,
                        BatchError::Unavailable(_)
                            | BatchError::MismatchedReceipt
                            | BatchError::Expired
                            | BatchError::Storage(_)
                    ) && client
                        .status()
                        .ok()
                        .and_then(|s| s.pending_report_id)
                        .as_deref()
                        == Some(report_id.as_str());
                    let state = if matches!(error, BatchError::GenerationCapacity { .. }) {
                        Some("refused_before_acceptance")
                    } else if pending {
                        Some("outcome_unknown")
                    } else {
                        None
                    };
                    let staged = if pending {
                        Some(true)
                    } else if matches!(error, BatchError::GenerationCapacity { .. }) {
                        Some(false)
                    } else {
                        None
                    };
                    let mut reply = serde_json::json!({"report_id":report_id,"error":error.to_string(),"code":error.code(),"delivery_state":state,"durably_staged":staged});
                    if let BatchError::GenerationCapacity { used, limit } = error {
                        reply["capacity"] = serde_json::json!({"used":used,"limit":limit});
                    }
                    reply
                }
            }
        }
        Ok(Command::Flush { timeout_ms }) => {
            let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(30000));
            match client.flush_local(deadline) {
                Ok(Some(receipt)) => {
                    serde_json::json!({"delivery_state":"custody_confirmed","projection":"unchecked","receipt":receipt})
                }
                Ok(None) => serde_json::json!({"receipt":null}),
                Err(error) => {
                    let pending = client
                        .status()
                        .ok()
                        .and_then(|s| s.pending_report_id)
                        .is_some();
                    let mut result = error_reply(error);
                    if pending {
                        result["delivery_state"] = "outcome_unknown".into();
                    }
                    result
                }
            }
        }
        Ok(Command::ReportStatus { report_id }) => match client.report_status(&report_id) {
            Ok(status) => serde_json::json!({"report_status":status}),
            Err(error) => error_reply(error),
        },
        Ok(Command::Status) => match client.status() {
            Ok(status) => serde_json::json!({"status":status}),
            Err(error) => error_reply(error),
        },
        Err(error) => error_reply(BatchError::Invalid(error)),
    }
}

#[cfg(test)]
mod tests;
