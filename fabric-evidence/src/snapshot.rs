//! Bounded, immutable in-memory experiment snapshots and observational comparisons.
//!
//! Callers must bound decoding and staging before constructing these values.
//! Identifiers and digest claims do not establish custody or authenticity.
use crate::evidence_manifest::ImmutableReference;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_CASES: usize = 256;
pub const MAX_ID_BYTES: usize = 128;

fn valid_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_ID_BYTES && !value.chars().any(char::is_control)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    InvalidIdentifier,
    InvalidReference,
    InvalidRoster,
    CoverageMismatch,
    IncompatiblePopulation,
    IncompatibleDefinition,
    InvalidSupplement,
}

/// Aggregation describes the reported measurement; no statistical property is inferred.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Aggregation {
    Count,
    Sum,
    Mean,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeasurementDefinition {
    evaluator: ImmutableReference,
    configuration: ImmutableReference,
    unit: String,
    aggregation: Aggregation,
}
impl MeasurementDefinition {
    pub fn new(
        evaluator: ImmutableReference,
        configuration: ImmutableReference,
        unit: String,
        aggregation: Aggregation,
    ) -> Result<Self, SnapshotError> {
        evaluator
            .validate()
            .map_err(|_| SnapshotError::InvalidReference)?;
        configuration
            .validate()
            .map_err(|_| SnapshotError::InvalidReference)?;
        if !valid_text(&unit)
            || matches!(&aggregation, Aggregation::Other(name) if !valid_text(name))
        {
            return Err(SnapshotError::InvalidIdentifier);
        }
        Ok(Self {
            evaluator,
            configuration,
            unit,
            aggregation,
        })
    }
    pub fn evaluator(&self) -> &ImmutableReference {
        &self.evaluator
    }
    pub fn configuration(&self) -> &ImmutableReference {
        &self.configuration
    }
    pub fn unit(&self) -> &str {
        &self.unit
    }
    pub fn aggregation(&self) -> &Aggregation {
        &self.aggregation
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownReason {
    Missing,
    Unavailable,
    Redacted,
    Unknown,
}

/// Exactness is an attributed result, not a calculated significance claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Known { value: i64, exact: bool },
    Unknown(UnknownReason),
}

/// After construction, the roster, cutoff and every outcome are immutable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    id: String,
    cutoff: String,
    supplements: Option<String>,
    definition: MeasurementDefinition,
    roster: BTreeSet<String>,
    outcomes: BTreeMap<String, Outcome>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counts {
    pub enrolled: usize,
    pub known: usize,
    pub exact: usize,
    pub unknown: usize,
}

impl Snapshot {
    pub fn new(
        id: String,
        cutoff: String,
        supplements: Option<String>,
        definition: MeasurementDefinition,
        roster: BTreeSet<String>,
        outcomes: BTreeMap<String, Outcome>,
    ) -> Result<Self, SnapshotError> {
        if !valid_text(&id)
            || !valid_text(&cutoff)
            || supplements
                .as_ref()
                .is_some_and(|p| !valid_text(p) || p == &id)
        {
            return Err(SnapshotError::InvalidIdentifier);
        }
        if roster.is_empty() || roster.len() > MAX_CASES || roster.iter().any(|id| !valid_text(id))
        {
            return Err(SnapshotError::InvalidRoster);
        }
        if outcomes.len() != roster.len() || !outcomes.keys().eq(roster.iter()) {
            return Err(SnapshotError::CoverageMismatch);
        }
        Ok(Self {
            id,
            cutoff,
            supplements,
            definition,
            roster,
            outcomes,
        })
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn cutoff(&self) -> &str {
        &self.cutoff
    }
    pub fn supplements(&self) -> Option<&str> {
        self.supplements.as_deref()
    }
    pub fn definition(&self) -> &MeasurementDefinition {
        &self.definition
    }
    pub fn roster(&self) -> &BTreeSet<String> {
        &self.roster
    }
    pub fn outcomes(&self) -> &BTreeMap<String, Outcome> {
        &self.outcomes
    }
    pub fn counts(&self) -> Counts {
        let known = self
            .outcomes
            .values()
            .filter(|o| matches!(o, Outcome::Known { .. }))
            .count();
        let exact = self
            .outcomes
            .values()
            .filter(|o| matches!(o, Outcome::Known { exact: true, .. }))
            .count();
        Counts {
            enrolled: self.roster.len(),
            known,
            exact,
            unknown: self.roster.len() - known,
        }
    }
    /// Checks the declared link and distinct boundaries, not temporal ordering.
    pub fn validate_supplement(&self, parent: &Snapshot) -> Result<(), SnapshotError> {
        if self.supplements.as_deref() != Some(parent.id())
            || self.cutoff == parent.cutoff
            || self.id == parent.id
        {
            return Err(SnapshotError::InvalidSupplement);
        }
        self.compatible(parent)
    }
    fn compatible(&self, other: &Snapshot) -> Result<(), SnapshotError> {
        if self.roster != other.roster {
            return Err(SnapshotError::IncompatiblePopulation);
        }
        if self.definition != other.definition {
            return Err(SnapshotError::IncompatibleDefinition);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Observation {
    Comparable {
        baseline: i64,
        candidate: i64,
        baseline_exact: bool,
        candidate_exact: bool,
        difference: i128,
    },
    Unavailable {
        baseline: Outcome,
        candidate: Outcome,
    },
}

/// Preserves the declared denominator and exposes missingness on either side.
pub fn compare(
    baseline: &Snapshot,
    candidate: &Snapshot,
) -> Result<BTreeMap<String, Observation>, SnapshotError> {
    baseline.compatible(candidate)?;
    Ok(baseline
        .roster
        .iter()
        .map(|id| {
            let left = &baseline.outcomes[id];
            let right = &candidate.outcomes[id];
            let observation = match (left, right) {
                (
                    Outcome::Known {
                        value: a,
                        exact: a_exact,
                    },
                    Outcome::Known {
                        value: b,
                        exact: b_exact,
                    },
                ) => Observation::Comparable {
                    baseline: *a,
                    candidate: *b,
                    baseline_exact: *a_exact,
                    candidate_exact: *b_exact,
                    difference: i128::from(*b) - i128::from(*a),
                },
                _ => Observation::Unavailable {
                    baseline: left.clone(),
                    candidate: right.clone(),
                },
            };
            (id.clone(), observation)
        })
        .collect())
}
