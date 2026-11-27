//! Reference-only v1 snapshot payloads inside validated application results.
//! References are claims until an authorized adapter verifies exact readback.
use crate::application_result::{
    ApplicationPayloadDigest, ApplicationResult, ApplicationResultError, ReportScope,
};
use crate::evidence_manifest::{DigestRepresentation, ImmutableReference, ProvenancedTime};
use crate::snapshot::{Aggregation, MeasurementDefinition, UnknownReason};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::num::NonZeroU64;

pub const SCHEMA: &str = "fabric.snapshot.v1";
pub const MAX_DURABLE_CASES: usize = 16;
const MAX_ID_BYTES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DurableError {
    Application(ApplicationResultError),
    InvalidSchema,
    InvalidPayload,
    InvalidReference,
    InvalidCutoff,
    InvalidCases,
    InvalidParent,
    Incompatible,
}
impl From<ApplicationResultError> for DurableError {
    fn from(e: ApplicationResultError) -> Self {
        Self::Application(e)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Cutoff {
    pub source: ImmutableReference,
    pub ordinal: u64,
    pub time: ProvenancedTime,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub evaluator: ImmutableReference,
    pub configuration: ImmutableReference,
    pub unit: String,
    pub aggregation: Aggregation,
}

/// Owner-scoped report and canonical application payload claims. The optional
/// source ID is a check against a later qualified receipt, not a receipt itself.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ApplicationReference {
    pub report_id: String,
    pub payload_digest: ApplicationPayloadDigest,
    pub expected_source_id: Option<NonZeroU64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaseEvidence {
    Known { reference: ApplicationReference },
    Unknown { reason: UnknownReason },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub id: String,
    pub evidence: CaseEvidence,
}

/// Input must be in strictly increasing UTF-8 case-ID order; it is never sorted
/// implicitly. The parent ID exists only in the ApplicationResult envelope.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    schema: String,
    pub cutoff: Cutoff,
    pub definition: Definition,
    pub cases: Vec<Case>,
    pub parent_payload_digest: Option<ApplicationPayloadDigest>,
}
impl Payload {
    pub fn new(
        cutoff: Cutoff,
        definition: Definition,
        cases: Vec<Case>,
        parent_payload_digest: Option<ApplicationPayloadDigest>,
    ) -> Result<Self, DurableError> {
        let payload = Self {
            schema: SCHEMA.into(),
            cutoff,
            definition,
            cases,
            parent_payload_digest,
        };
        payload.validate()?;
        Ok(payload)
    }
    fn validate(&self) -> Result<(), DurableError> {
        if self.schema != SCHEMA {
            return Err(DurableError::InvalidSchema);
        }
        self.cutoff
            .source
            .validate()
            .map_err(|_| DurableError::InvalidCutoff)?;
        if !text(&self.cutoff.time.clock.clock_id, 128)
            || !text(&self.cutoff.time.clock.source, 256)
        {
            return Err(DurableError::InvalidCutoff);
        }
        MeasurementDefinition::new(
            self.definition.evaluator.clone(),
            self.definition.configuration.clone(),
            self.definition.unit.clone(),
            self.definition.aggregation.clone(),
        )
        .map_err(|_| DurableError::InvalidReference)?;
        if self.cases.is_empty() || self.cases.len() > MAX_DURABLE_CASES {
            return Err(DurableError::InvalidCases);
        }
        let mut prior: Option<&str> = None;
        let mut reports = BTreeSet::new();
        for case in &self.cases {
            if !text(&case.id, MAX_ID_BYTES) || prior.is_some_and(|p| p >= case.id.as_str()) {
                return Err(DurableError::InvalidCases);
            }
            prior = Some(&case.id);
            if let CaseEvidence::Known { reference } = &case.evidence {
                if !uuid(&reference.report_id)
                    || !reports.insert(&reference.report_id)
                    || !digest(&reference.payload_digest)
                {
                    return Err(DurableError::InvalidReference);
                }
            }
        }
        if self
            .parent_payload_digest
            .as_ref()
            .is_some_and(|d| !digest(d))
        {
            return Err(DurableError::InvalidReference);
        }
        Ok(())
    }
}
fn text(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}
fn digest(d: &ApplicationPayloadDigest) -> bool {
    d.0.validate().is_ok() && d.0.representation == DigestRepresentation::ExactBytes
}
fn uuid(s: &str) -> bool {
    s.len() == 36
        && s != "00000000-0000-0000-0000-000000000000"
        && s.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        })
}

#[derive(Clone, Debug)]
pub struct DurableSnapshot {
    result: ApplicationResult,
    payload: Payload,
}
impl DurableSnapshot {
    /// The application-result constructor supplies the canonical profile,
    /// digest and hard payload/shape bounds; no parallel canonicalizer is used.
    pub fn create(
        scope: ReportScope,
        report_id: String,
        parent_report_id: Option<String>,
        completion: ProvenancedTime,
        payload: Payload,
    ) -> Result<Self, DurableError> {
        payload.validate()?;
        let artifact = serde_json::to_vec(&payload).map_err(|_| DurableError::InvalidPayload)?;
        let result = ApplicationResult::new(
            scope,
            report_id,
            parent_report_id,
            SCHEMA.into(),
            completion,
            &artifact,
        )?;
        Self::from_result(result)
    }
    /// Consumes an already validated result and reuses its canonical payload.
    pub fn from_result(result: ApplicationResult) -> Result<Self, DurableError> {
        if result.schema() != SCHEMA {
            return Err(DurableError::InvalidSchema);
        }
        let payload: Payload = serde_json::from_slice(&result.canonical_payload()?)
            .map_err(|_| DurableError::InvalidPayload)?;
        payload.validate()?;
        if result.parent_report_id().is_some() != payload.parent_payload_digest.is_some() {
            return Err(DurableError::InvalidParent);
        }
        Ok(Self { result, payload })
    }
    pub fn result(&self) -> &ApplicationResult {
        &self.result
    }
    pub fn payload(&self) -> &Payload {
        &self.payload
    }
    /// Parent ordering uses the same immutable cutoff source and ordinal only;
    /// timestamps from different sources or clocks are never ordered here.
    pub fn validate_supplement(&self, parent: &Self) -> Result<(), DurableError> {
        if self.result.parent_report_id() != Some(parent.result.report_id())
            || self.result.scope() != parent.result.scope()
            || self.payload.parent_payload_digest.as_ref()
                != Some(parent.result.application_payload_digest())
        {
            return Err(DurableError::InvalidParent);
        }
        if self.payload.cutoff.source != parent.payload.cutoff.source
            || self.payload.cutoff.ordinal <= parent.payload.cutoff.ordinal
        {
            return Err(DurableError::InvalidCutoff);
        }
        if self.payload.definition != parent.payload.definition
            || !self.payload.cases.iter().map(|c| &c.id).eq(parent
                .payload
                .cases
                .iter()
                .map(|c| &c.id))
        {
            return Err(DurableError::Incompatible);
        }
        Ok(())
    }
    /// Pure claim matching only; the caller must authenticate and qualify the
    /// readback and receipt before interpreting a successful match as evidence.
    pub fn match_case(
        &self,
        case_id: &str,
        resolved: &ApplicationResult,
        source_id: Option<NonZeroU64>,
    ) -> Result<(), DurableError> {
        let Some(Case {
            evidence: CaseEvidence::Known { reference },
            ..
        }) = self.payload.cases.iter().find(|case| case.id == case_id)
        else {
            return Err(DurableError::InvalidReference);
        };
        if resolved.scope() != self.result.scope()
            || resolved.report_id() != reference.report_id
            || resolved.application_payload_digest() != &reference.payload_digest
            || reference
                .expected_source_id
                .is_some_and(|id| Some(id) != source_id)
        {
            return Err(DurableError::InvalidReference);
        }
        Ok(())
    }
}
