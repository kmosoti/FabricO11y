//! Bounded reported attempt observations. Authentication belongs to the caller.
use crate::application_result::ApplicationResult;
use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "fabric.prior-failure.attempt-observation.v1";
pub const MAX_FIELD_BYTES: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttemptIdentity {
    pub attempt: String,
    pub candidate: String,
    pub intended_strategy: String,
    pub actual_artifact: String,
    pub diagnosis: String,
}
impl AttemptIdentity {
    pub fn validate(&self) -> bool {
        [
            &self.attempt,
            &self.candidate,
            &self.intended_strategy,
            &self.actual_artifact,
            &self.diagnosis,
        ]
        .iter()
        .all(|s| !s.is_empty() && s.len() <= MAX_FIELD_BYTES && !s.chars().any(char::is_control))
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalState {
    ObservedFailure,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttemptObservation {
    pub schema: String,
    pub attempt: String,
    pub candidate: String,
    pub intended_strategy: String,
    pub actual_artifact: String,
    pub diagnosis: String,
    pub state: TerminalState,
}
impl AttemptObservation {
    pub fn identity(&self) -> AttemptIdentity {
        AttemptIdentity {
            attempt: self.attempt.clone(),
            candidate: self.candidate.clone(),
            intended_strategy: self.intended_strategy.clone(),
            actual_artifact: self.actual_artifact.clone(),
            diagnosis: self.diagnosis.clone(),
        }
    }
    /// Decode only the named schema and bind every field to caller expectations.
    pub fn from_result(
        result: &ApplicationResult,
        expected: &AttemptIdentity,
    ) -> Result<Self, &'static str> {
        if result.schema() != SCHEMA || !expected.validate() {
            return Err("invalid attempt observation");
        }
        let bytes = result
            .canonical_payload()
            .map_err(|_| "invalid attempt observation")?;
        let observation: Self =
            serde_json::from_slice(&bytes).map_err(|_| "invalid attempt observation")?;
        if observation.schema != SCHEMA
            || !observation.identity().validate()
            || &observation.identity() != expected
        {
            return Err("invalid attempt observation");
        }
        Ok(observation)
    }
}
