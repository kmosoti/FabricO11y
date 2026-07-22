//! Typed forms of the normative JSON evidence envelopes.

use std::collections::{BTreeMap, BTreeSet};

use fabric_time::{LogicalTime, OrderingPoint, Timestamp};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const OBSERVATION_VERSION: &str = "fabric.observation/v1";
pub const CORRECTION_VERSION: &str = "fabric.correction/v1";
pub const FRONTIER_VERSION: &str = "fabric.frontier/v1";
pub const ANSWER_VERSION: &str = "fabric.answer/v1";

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Producer {
    pub producer_id: String,
    pub stream_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpistemicClass {
    ObservedFact,
    DerivedConclusion,
    Correlation,
    Assumption,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Interpretation {
    ObservedFact,
    DerivedConclusion {
        derivation_id: String,
        input_ids: BTreeSet<String>,
    },
    Correlation {
        related_ids: BTreeSet<String>,
    },
    Assumption {
        statement: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum InterpretationKind {
    ObservedFact,
    DerivedConclusion,
    Correlation,
    Assumption,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservedFactWire {
    kind: InterpretationKind,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DerivedConclusionWire {
    kind: InterpretationKind,
    derivation_id: String,
    #[serde(deserialize_with = "deserialize_unique_set")]
    input_ids: BTreeSet<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CorrelationWire {
    kind: InterpretationKind,
    #[serde(deserialize_with = "deserialize_unique_set")]
    related_ids: BTreeSet<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AssumptionWire {
    kind: InterpretationKind,
    statement: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum InterpretationWire {
    ObservedFact(ObservedFactWire),
    DerivedConclusion(DerivedConclusionWire),
    Correlation(CorrelationWire),
    Assumption(AssumptionWire),
}

impl<'de> Deserialize<'de> for Interpretation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match InterpretationWire::deserialize(deserializer)? {
            InterpretationWire::ObservedFact(wire)
                if wire.kind == InterpretationKind::ObservedFact =>
            {
                Ok(Self::ObservedFact)
            }
            InterpretationWire::DerivedConclusion(wire)
                if wire.kind == InterpretationKind::DerivedConclusion =>
            {
                Ok(Self::DerivedConclusion {
                    derivation_id: wire.derivation_id,
                    input_ids: wire.input_ids,
                })
            }
            InterpretationWire::Correlation(wire)
                if wire.kind == InterpretationKind::Correlation =>
            {
                Ok(Self::Correlation {
                    related_ids: wire.related_ids,
                })
            }
            InterpretationWire::Assumption(wire) if wire.kind == InterpretationKind::Assumption => {
                Ok(Self::Assumption {
                    statement: wire.statement,
                })
            }
            _ => Err(D::Error::custom(
                "interpretation kind does not match its variant fields",
            )),
        }
    }
}

impl Interpretation {
    #[must_use]
    pub const fn epistemic_class(&self) -> EpistemicClass {
        match self {
            Self::ObservedFact => EpistemicClass::ObservedFact,
            Self::DerivedConclusion { .. } => EpistemicClass::DerivedConclusion,
            Self::Correlation { .. } => EpistemicClass::Correlation,
            Self::Assumption { .. } => EpistemicClass::Assumption,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relations {
    #[serde(deserialize_with = "deserialize_unique_set")]
    pub parents: BTreeSet<String>,
    #[serde(deserialize_with = "deserialize_unique_set")]
    pub links: BTreeSet<String>,
    #[serde(deserialize_with = "deserialize_unique_set")]
    pub correlations: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Integrity {
    pub algorithm: String,
    pub digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    pub source_type: String,
    pub source_id: String,
    pub schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub api_version: String,
    pub observation_id: String,
    pub epistemic_class: EpistemicClass,
    pub interpretation: Interpretation,
    pub producer: Producer,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub observed_at: Option<Timestamp>,
    pub observed_by_at: Timestamp,
    pub recorded_at: Timestamp,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub producer_sequence: Option<u64>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub logical_time: Option<LogicalTime>,
    pub relations: Relations,
    pub payload: Value,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub payload_ref: Option<String>,
    pub integrity: Integrity,
    pub classification: String,
    pub origin: Origin,
    pub attributes: BTreeMap<String, Value>,
}

impl Observation {
    pub fn validate(&self) -> Result<(), SchemaError> {
        require_version(&self.api_version, OBSERVATION_VERSION)?;
        require_id(&self.observation_id, "observation_id")?;
        require_producer(&self.producer)?;
        require_logical_time(self.logical_time.as_ref(), "logical_time.clock_id")?;
        if self.epistemic_class != self.interpretation.epistemic_class() {
            return Err(SchemaError::MixedEpistemicClass);
        }
        match &self.interpretation {
            Interpretation::DerivedConclusion {
                derivation_id,
                input_ids,
            } => {
                require_id(derivation_id, "derivation_id")?;
                require_non_empty(input_ids, "input_ids")?;
                require_ids(input_ids, "input_ids")?;
            }
            Interpretation::Correlation { related_ids } => {
                if related_ids.len() < 2 {
                    return Err(SchemaError::TooFewRelatedRecords);
                }
                require_ids(related_ids, "related_ids")?;
            }
            Interpretation::Assumption { statement } => {
                require_non_empty_string(statement, "statement")?;
            }
            Interpretation::ObservedFact => {}
        }
        require_ids(&self.relations.parents, "relations.parents")?;
        require_ids(&self.relations.links, "relations.links")?;
        require_ids(&self.relations.correlations, "relations.correlations")?;
        if let Some(payload_ref) = &self.payload_ref {
            require_non_empty_string(payload_ref, "payload_ref")?;
        }
        if self.integrity.algorithm != "sha256"
            || self.integrity.digest.len() != 64
            || !self
                .integrity
                .digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(SchemaError::InvalidIntegrity);
        }
        require_classification(&self.classification)?;
        require_non_empty_string(&self.origin.source_type, "origin.source_type")?;
        require_id(&self.origin.source_id, "origin.source_id")?;
        require_non_empty_string(&self.origin.schema, "origin.schema")?;
        Ok(())
    }

    #[must_use]
    pub fn ordering_point(&self) -> OrderingPoint {
        OrderingPoint {
            record_id: self.observation_id.clone(),
            producer_id: self.producer.producer_id.clone(),
            stream_id: self.producer.stream_id.clone(),
            producer_sequence: self.producer_sequence,
            logical_time: self.logical_time.clone(),
            parents: self.relations.parents.clone(),
            observed_at: self.observed_at.clone(),
            observed_by_at: self.observed_by_at.clone(),
            recorded_at: self.recorded_at.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrectionOperation {
    Retraction,
    Replacement,
    Qualification,
    Deduplication,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Correction {
    pub api_version: String,
    pub correction_id: String,
    pub producer: Producer,
    pub observed_by_at: Timestamp,
    pub recorded_at: Timestamp,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub producer_sequence: Option<u64>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub logical_time: Option<LogicalTime>,
    pub operation: CorrectionOperation,
    #[serde(deserialize_with = "deserialize_unique_set")]
    pub targets: BTreeSet<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_unique_set",
        skip_serializing_if = "Option::is_none"
    )]
    pub replacement_ids: Option<BTreeSet<String>>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub qualification: Option<String>,
    pub reason: String,
    #[serde(deserialize_with = "deserialize_unique_set")]
    pub provenance: BTreeSet<String>,
    pub classification: String,
}

impl Correction {
    pub fn validate(&self) -> Result<(), SchemaError> {
        require_version(&self.api_version, CORRECTION_VERSION)?;
        require_id(&self.correction_id, "correction_id")?;
        require_producer(&self.producer)?;
        require_logical_time(self.logical_time.as_ref(), "logical_time.clock_id")?;
        require_non_empty(&self.targets, "targets")?;
        require_ids(&self.targets, "targets")?;
        require_non_empty_string(&self.reason, "reason")?;
        require_ids(&self.provenance, "provenance")?;
        require_classification(&self.classification)?;
        match self.operation {
            CorrectionOperation::Retraction => {
                require_absent(&self.replacement_ids, "replacement_ids")?;
                require_absent(&self.qualification, "qualification")?;
            }
            CorrectionOperation::Replacement => {
                let replacements =
                    require_present_non_empty(&self.replacement_ids, "replacement_ids")?;
                require_ids(replacements, "replacement_ids")?;
                require_absent(&self.qualification, "qualification")?;
            }
            CorrectionOperation::Qualification => {
                require_absent(&self.replacement_ids, "replacement_ids")?;
                let qualification = self
                    .qualification
                    .as_deref()
                    .ok_or(SchemaError::MissingField("qualification"))?;
                require_non_empty_string(qualification, "qualification")?;
            }
            CorrectionOperation::Deduplication => {
                let replacements =
                    require_present_non_empty(&self.replacement_ids, "replacement_ids")?;
                if replacements.len() != 1 {
                    return Err(SchemaError::DeduplicationCanonicalCount);
                }
                require_ids(replacements, "replacement_ids")?;
                require_absent(&self.qualification, "qualification")?;
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn ordering_point(&self) -> OrderingPoint {
        OrderingPoint {
            record_id: self.correction_id.clone(),
            producer_id: self.producer.producer_id.clone(),
            stream_id: self.producer.stream_id.clone(),
            producer_sequence: self.producer_sequence,
            logical_time: self.logical_time.clone(),
            parents: BTreeSet::new(),
            observed_at: None,
            observed_by_at: self.observed_by_at.clone(),
            recorded_at: self.recorded_at.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SequenceGap {
    pub start: u64,
    pub end: u64,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SamplingMode {
    Complete,
    Sampled,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sampling {
    pub mode: SamplingMode,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub rate: Option<f64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProducerState {
    Active,
    Paused,
    Retired,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frontier {
    pub api_version: String,
    pub frontier_id: String,
    pub producer: Producer,
    pub as_of_recorded_at: Timestamp,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub producer_sequence_through: Option<u64>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub logical_time_through: Option<LogicalTime>,
    pub known_gaps: Vec<SequenceGap>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub retention_start: Option<Timestamp>,
    pub sampling: Sampling,
    pub producer_state: ProducerState,
    pub classification: String,
}

impl Frontier {
    pub fn validate(&self) -> Result<(), SchemaError> {
        require_version(&self.api_version, FRONTIER_VERSION)?;
        require_id(&self.frontier_id, "frontier_id")?;
        require_producer(&self.producer)?;
        require_logical_time(
            self.logical_time_through.as_ref(),
            "logical_time_through.clock_id",
        )?;
        for gap in &self.known_gaps {
            if gap.start > gap.end {
                return Err(SchemaError::InvalidSequenceGap {
                    start: gap.start,
                    end: gap.end,
                });
            }
            require_non_empty_string(&gap.reason, "gap.reason")?;
        }
        match (self.sampling.mode, self.sampling.rate) {
            (SamplingMode::Sampled, Some(rate)) if rate > 0.0 && rate <= 1.0 => {}
            (SamplingMode::Sampled, _) => return Err(SchemaError::InvalidSampling),
            (_, None) => {}
            (_, Some(_)) => return Err(SchemaError::InvalidSampling),
        }
        require_classification(&self.classification)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvenanceItem {
    pub record_id: String,
    pub role: String,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_unique_set",
        skip_serializing_if = "Option::is_none"
    )]
    pub fields: Option<BTreeSet<String>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceCutoff {
    pub recorded_at: Timestamp,
    #[serde(deserialize_with = "deserialize_unique_set")]
    pub segment_ids: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrontierReference {
    pub frontier_id: String,
    pub producer: Producer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    Complete,
    Partial,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Coverage {
    pub status: CoverageStatus,
    pub requested_sources: Vec<Producer>,
    pub covered_sources: Vec<Producer>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerAssumption {
    pub assumption_id: String,
    pub statement: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerConflict {
    pub conflict_id: String,
    #[serde(deserialize_with = "deserialize_unique_set")]
    pub record_ids: BTreeSet<String>,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Derivation {
    pub name: String,
    pub version: String,
    #[serde(deserialize_with = "deserialize_unique_set")]
    pub input_ids: BTreeSet<String>,
    pub parameters: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Answer {
    pub api_version: String,
    pub answer_id: String,
    pub evaluated_at: Timestamp,
    pub result: Value,
    pub provenance: Vec<ProvenanceItem>,
    pub source_cutoff: SourceCutoff,
    pub frontiers: Vec<FrontierReference>,
    #[serde(deserialize_with = "deserialize_unique_vec")]
    pub missing_sources: Vec<Producer>,
    pub coverage: Coverage,
    pub assumptions: Vec<AnswerAssumption>,
    pub conflicts: Vec<AnswerConflict>,
    pub derivation: Derivation,
}

impl Answer {
    pub fn validate(&self) -> Result<(), SchemaError> {
        require_version(&self.api_version, ANSWER_VERSION)?;
        require_id(&self.answer_id, "answer_id")?;
        for item in &self.provenance {
            require_id(&item.record_id, "provenance.record_id")?;
            require_non_empty_string(&item.role, "provenance.role")?;
        }
        require_ids(&self.source_cutoff.segment_ids, "source_cutoff.segment_ids")?;
        for frontier in &self.frontiers {
            require_id(&frontier.frontier_id, "frontiers.frontier_id")?;
            require_producer(&frontier.producer)?;
        }
        for producer in &self.missing_sources {
            require_producer(producer)?;
        }
        for producer in &self.coverage.requested_sources {
            require_producer(producer)?;
        }
        for producer in &self.coverage.covered_sources {
            require_producer(producer)?;
        }
        for assumption in &self.assumptions {
            require_id(&assumption.assumption_id, "assumptions.assumption_id")?;
            require_non_empty_string(&assumption.statement, "assumptions.statement")?;
        }
        for conflict in &self.conflicts {
            require_id(&conflict.conflict_id, "conflicts.conflict_id")?;
            if conflict.record_ids.len() < 2 {
                return Err(SchemaError::TooFewConflictRecords);
            }
            require_ids(&conflict.record_ids, "conflicts.record_ids")?;
            require_non_empty_string(&conflict.description, "conflicts.description")?;
        }
        require_non_empty_string(&self.derivation.name, "derivation.name")?;
        require_non_empty_string(&self.derivation.version, "derivation.version")?;
        require_ids(&self.derivation.input_ids, "derivation.input_ids")?;
        Ok(())
    }
}

/// An appendable semantic record. Answers are derived output, not admitted evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EvidenceRecord {
    Observation(Box<Observation>),
    Correction(Correction),
    Frontier(Frontier),
}

impl From<Observation> for EvidenceRecord {
    fn from(record: Observation) -> Self {
        Self::Observation(Box::new(record))
    }
}

impl EvidenceRecord {
    #[must_use]
    pub fn record_id(&self) -> &str {
        match self {
            Self::Observation(record) => &record.observation_id,
            Self::Correction(record) => &record.correction_id,
            Self::Frontier(record) => &record.frontier_id,
        }
    }

    #[must_use]
    pub fn recorded_at(&self) -> &Timestamp {
        match self {
            Self::Observation(record) => &record.recorded_at,
            Self::Correction(record) => &record.recorded_at,
            Self::Frontier(record) => &record.as_of_recorded_at,
        }
    }

    pub fn validate(&self) -> Result<(), SchemaError> {
        match self {
            Self::Observation(record) => record.validate(),
            Self::Correction(record) => record.validate(),
            Self::Frontier(record) => record.validate(),
        }
    }
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum SchemaError {
    #[error("unsupported envelope version {actual}; expected {expected}")]
    UnsupportedVersion {
        actual: String,
        expected: &'static str,
    },
    #[error("empty required field: {0}")]
    EmptyField(&'static str),
    #[error("invalid identifier: {0}")]
    InvalidIdentifier(&'static str),
    #[error("classification must be a lowercase policy label")]
    InvalidClassification,
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("field is not permitted for this variant: {0}")]
    UnexpectedField(&'static str),
    #[error("epistemic_class and interpretation.kind do not match")]
    MixedEpistemicClass,
    #[error("correlation requires at least two related records")]
    TooFewRelatedRecords,
    #[error("answer conflict requires at least two record ids")]
    TooFewConflictRecords,
    #[error("integrity must be lowercase SHA-256")]
    InvalidIntegrity,
    #[error("invalid sequence gap {start}..{end}")]
    InvalidSequenceGap { start: u64, end: u64 },
    #[error("sampled frontiers require a rate in (0, 1], other modes require null")]
    InvalidSampling,
    #[error("deduplication requires exactly one canonical replacement")]
    DeduplicationCanonicalCount,
}

fn deserialize_required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

fn deserialize_optional_non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn deserialize_unique_set<'de, D, T>(deserializer: D) -> Result<BTreeSet<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Ord,
{
    let values = Vec::<T>::deserialize(deserializer)?;
    let item_count = values.len();
    let values = values.into_iter().collect::<BTreeSet<_>>();
    if values.len() == item_count {
        Ok(values)
    } else {
        Err(D::Error::custom("array elements must be unique"))
    }
}

fn deserialize_optional_unique_set<'de, D, T>(
    deserializer: D,
) -> Result<Option<BTreeSet<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Ord,
{
    deserialize_unique_set(deserializer).map(Some)
}

fn deserialize_unique_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Ord,
{
    let values = Vec::<T>::deserialize(deserializer)?;
    if values.iter().collect::<BTreeSet<_>>().len() == values.len() {
        Ok(values)
    } else {
        Err(D::Error::custom("array elements must be unique"))
    }
}

fn require_version(actual: &str, expected: &'static str) -> Result<(), SchemaError> {
    if actual == expected {
        Ok(())
    } else {
        Err(SchemaError::UnsupportedVersion {
            actual: actual.to_owned(),
            expected,
        })
    }
}

fn require_id(value: &str, field: &'static str) -> Result<(), SchemaError> {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return Err(SchemaError::InvalidIdentifier(field));
    };
    if value.len() <= 256
        && first.is_ascii_alphanumeric()
        && bytes.all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'/' | b'-')
        })
    {
        Ok(())
    } else {
        Err(SchemaError::InvalidIdentifier(field))
    }
}

fn require_producer(producer: &Producer) -> Result<(), SchemaError> {
    require_id(&producer.producer_id, "producer_id")?;
    require_id(&producer.stream_id, "stream_id")
}

fn require_logical_time(
    logical_time: Option<&LogicalTime>,
    field: &'static str,
) -> Result<(), SchemaError> {
    if let Some(logical_time) = logical_time {
        require_id(&logical_time.clock_id, field)?;
    }
    Ok(())
}

fn require_ids<'a>(
    values: impl IntoIterator<Item = &'a String>,
    field: &'static str,
) -> Result<(), SchemaError> {
    for value in values {
        require_id(value, field)?;
    }
    Ok(())
}

fn require_non_empty_string(value: &str, field: &'static str) -> Result<(), SchemaError> {
    if value.is_empty() {
        Err(SchemaError::EmptyField(field))
    } else {
        Ok(())
    }
}

fn require_classification(value: &str) -> Result<(), SchemaError> {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return Err(SchemaError::InvalidClassification);
    };
    if value.len() <= 64
        && first.is_ascii_lowercase()
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
    {
        Ok(())
    } else {
        Err(SchemaError::InvalidClassification)
    }
}

fn require_non_empty<T>(values: &BTreeSet<T>, field: &'static str) -> Result<(), SchemaError> {
    if values.is_empty() {
        Err(SchemaError::MissingField(field))
    } else {
        Ok(())
    }
}

fn require_absent<T>(value: &Option<T>, field: &'static str) -> Result<(), SchemaError> {
    if value.is_some() {
        Err(SchemaError::UnexpectedField(field))
    } else {
        Ok(())
    }
}

fn require_present_non_empty<'a, T>(
    value: &'a Option<BTreeSet<T>>,
    field: &'static str,
) -> Result<&'a BTreeSet<T>, SchemaError> {
    value
        .as_ref()
        .filter(|values| !values.is_empty())
        .ok_or(SchemaError::MissingField(field))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation_value() -> Value {
        serde_json::from_str(include_str!(
            "../../../fixtures/golden/contracts/valid/observation.json"
        ))
        .expect("observation fixture must be JSON")
    }

    #[test]
    fn canonical_contract_fixtures_deserialize_and_validate() {
        let observation: Observation = serde_json::from_str(include_str!(
            "../../../fixtures/golden/contracts/valid/observation.json"
        ))
        .expect("observation fixture must deserialize");
        let correction: Correction = serde_json::from_str(include_str!(
            "../../../fixtures/golden/contracts/valid/correction.json"
        ))
        .expect("correction fixture must deserialize");
        let frontier: Frontier = serde_json::from_str(include_str!(
            "../../../fixtures/golden/contracts/valid/frontier.json"
        ))
        .expect("frontier fixture must deserialize");
        let answer: Answer = serde_json::from_str(include_str!(
            "../../../fixtures/golden/contracts/valid/answer.json"
        ))
        .expect("answer fixture must deserialize");

        observation.validate().expect("valid observation");
        correction.validate().expect("valid correction");
        frontier.validate().expect("valid frontier");
        answer.validate().expect("valid answer");
    }

    #[test]
    fn class_mismatch_is_rejected_by_typed_validation() {
        let mut observation: Observation = serde_json::from_str(include_str!(
            "../../../fixtures/golden/contracts/valid/observation.json"
        ))
        .expect("observation fixture must deserialize");
        observation.epistemic_class = EpistemicClass::Assumption;
        assert_eq!(
            observation.validate(),
            Err(SchemaError::MixedEpistemicClass)
        );
    }

    #[test]
    fn closed_objects_reject_unknown_properties() {
        let mut top_level = observation_value();
        top_level["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<Observation>(top_level).is_err());

        let mut nested = observation_value();
        nested["producer"]["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<Observation>(nested).is_err());

        let mut interpretation = observation_value();
        interpretation["interpretation"]["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<Observation>(interpretation).is_err());
    }

    #[test]
    fn required_nullable_fields_distinguish_omission_from_null() {
        let mut missing = observation_value();
        missing
            .as_object_mut()
            .expect("observation is an object")
            .remove("observed_at");
        assert!(serde_json::from_value::<Observation>(missing).is_err());

        let mut explicit_null = observation_value();
        explicit_null["observed_at"] = Value::Null;
        let observation = serde_json::from_value::<Observation>(explicit_null)
            .expect("required nullable field accepts null");
        assert_eq!(observation.observed_at, None);
    }

    #[test]
    fn optional_non_nullable_fields_reject_explicit_null() {
        let mut explicit_null = observation_value();
        explicit_null["payload_ref"] = Value::Null;
        assert!(serde_json::from_value::<Observation>(explicit_null).is_err());

        let mut absent = observation_value();
        absent
            .as_object_mut()
            .expect("observation is an object")
            .remove("payload_ref");
        let observation =
            serde_json::from_value::<Observation>(absent).expect("optional field may be absent");
        assert_eq!(observation.payload_ref, None);
    }

    #[test]
    fn unique_arrays_reject_duplicates_instead_of_collapsing_them() {
        let mut observation = observation_value();
        observation["relations"]["links"] = serde_json::json!(["obs-bootstrap", "obs-bootstrap"]);
        assert!(serde_json::from_value::<Observation>(observation).is_err());

        let mut answer: Value = serde_json::from_str(include_str!(
            "../../../fixtures/golden/contracts/valid/answer.json"
        ))
        .expect("answer fixture must be JSON");
        let producer = serde_json::json!({"producer_id": "checkout-api", "stream_id": "trace"});
        answer["missing_sources"] = serde_json::json!([producer, producer]);
        assert!(serde_json::from_value::<Answer>(answer).is_err());
    }

    #[test]
    fn identifiers_and_classifications_follow_normative_grammar() {
        let mut invalid_identifier: Observation = serde_json::from_value(observation_value())
            .expect("observation fixture must deserialize");
        invalid_identifier.observation_id = "contains spaces".to_owned();
        assert_eq!(
            invalid_identifier.validate(),
            Err(SchemaError::InvalidIdentifier("observation_id"))
        );

        let mut invalid_classification: Observation = serde_json::from_value(observation_value())
            .expect("observation fixture must deserialize");
        invalid_classification.classification = "Internal".to_owned();
        assert_eq!(
            invalid_classification.validate(),
            Err(SchemaError::InvalidClassification)
        );
    }

    #[test]
    fn every_canonical_contract_round_trip_is_stable() {
        fn assert_round_trip<T>(json: &str)
        where
            T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
        {
            let value: T = serde_json::from_str(json).expect("fixture must deserialize");
            let first = serde_json::to_string(&value).expect("serialize fixture");
            let round_trip: T = serde_json::from_str(&first).expect("round trip fixture");
            assert_eq!(round_trip, value);
            assert_eq!(
                serde_json::to_string(&round_trip).expect("serialize round trip"),
                first
            );
        }

        assert_round_trip::<Observation>(include_str!(
            "../../../fixtures/golden/contracts/valid/observation.json"
        ));
        assert_round_trip::<Correction>(include_str!(
            "../../../fixtures/golden/contracts/valid/correction.json"
        ));
        assert_round_trip::<Frontier>(include_str!(
            "../../../fixtures/golden/contracts/valid/frontier.json"
        ));
        assert_round_trip::<Answer>(include_str!(
            "../../../fixtures/golden/contracts/valid/answer.json"
        ));
    }
}
