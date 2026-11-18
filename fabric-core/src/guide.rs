//! Offline guide catalog and candidate evidence wire format.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const CATALOG_PATH: &str = "verification/guide.json";
pub const CATALOG_BYTES: &[u8] = include_bytes!("../../verification/guide.json");
pub const MAX_GUIDE_BYTES: usize = 256 * 1024;
pub const SCHEMA: u32 = 1;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema: u32,
    pub topics: Vec<Topic>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Topic {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub scenarios: Vec<Scenario>,
    pub documentation: Vec<String>,
    pub proofs: Vec<Proof>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub description: String,
    pub command: String,
    pub expected: String,
    pub checks: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofKind {
    ExtractedRust,
    IndependentModel,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Proof {
    pub kind: ProofKind,
    pub path: String,
    pub theorem: String,
    pub checker: String,
    pub scope: String,
    pub assumptions: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub schema: u32,
    pub candidate: String,
    pub catalog_sha256: String,
    pub manifest_sha256: String,
    pub checks: Vec<CheckOutcome>,
    pub checkers: Vec<Checker>,
    pub artifacts: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckOutcome {
    pub id: String,
    pub status: CheckStatus,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Passed,
    Failed,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Checker {
    pub id: String,
    pub identity: String,
}

fn single_line(value: &str, max_bytes: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn bounded<T>(entries: &[T], max: usize) -> bool {
    !entries.is_empty() && entries.len() <= max
}

fn unique<'a>(values: impl IntoIterator<Item = &'a str>) -> bool {
    let mut seen = BTreeSet::new();
    values.into_iter().all(|value| seen.insert(value))
}

fn repository_path(path: &str) -> bool {
    path.len() <= 128
        && path.split('/').all(|part| {
            !matches!(part, "" | "." | "..")
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        })
}

fn hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn is_candidate_revision(value: &str) -> bool {
    hex(value, 40)
}

impl Catalog {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_GUIDE_BYTES {
            return Err("guide catalog exceeds 256 KiB".into());
        }
        let catalog: Self = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        if catalog.schema != SCHEMA
            || !bounded(&catalog.topics, 32)
            || !unique(catalog.topics.iter().map(|topic| topic.id.as_str()))
        {
            return Err("invalid guide catalog schema or topic set".into());
        }
        for topic in &catalog.topics {
            topic.validate()?;
        }
        Ok(catalog)
    }
}

impl Topic {
    fn validate(&self) -> Result<(), String> {
        if !id(&self.id)
            || !single_line(&self.title, 100)
            || !single_line(&self.summary, 500)
            || !bounded(&self.scenarios, 16)
            || !bounded(&self.documentation, 16)
            || !self.documentation.iter().all(|path| repository_path(path))
            || self.proofs.len() > 16
        {
            return Err(format!("invalid guide topic {}", self.id));
        }
        for scenario in &self.scenarios {
            if !single_line(&scenario.description, 200)
                || !single_line(&scenario.command, 500)
                || !single_line(&scenario.expected, 500)
                || !bounded(&scenario.checks, 16)
                || !scenario.checks.iter().all(|check| id(check))
                || !unique(scenario.checks.iter().map(String::as_str))
            {
                return Err(format!("invalid scenario in guide topic {}", self.id));
            }
        }
        for proof in &self.proofs {
            if !repository_path(&proof.path)
                || !proof.path.starts_with("proofs/")
                || !proof.path.ends_with(".lean")
                || !single_line(&proof.theorem, 128)
                || !proof
                    .theorem
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_'))
                || !id(&proof.checker)
                || !single_line(&proof.scope, 500)
                || !single_line(&proof.assumptions, 500)
            {
                return Err(format!("invalid proof in guide topic {}", self.id));
            }
        }
        Ok(())
    }
}

impl Evidence {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_GUIDE_BYTES {
            return Err("guide evidence exceeds 256 KiB".into());
        }
        let evidence: Self = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        if evidence.schema != SCHEMA
            || !is_candidate_revision(&evidence.candidate)
            || !hex(&evidence.catalog_sha256, 64)
            || !hex(&evidence.manifest_sha256, 64)
            || !bounded(&evidence.checks, 130)
            || !evidence.checks.iter().all(|check| id(&check.id))
            || !unique(evidence.checks.iter().map(|check| check.id.as_str()))
            || !bounded(&evidence.checkers, 16)
            || !evidence
                .checkers
                .iter()
                .all(|checker| id(&checker.id) && single_line(&checker.identity, 256))
            || !unique(evidence.checkers.iter().map(|checker| checker.id.as_str()))
            || !bounded(&evidence.artifacts, 16)
            || !evidence.artifacts.iter().all(|path| repository_path(path))
        {
            return Err("invalid guide evidence schema, identity, or records".into());
        }
        Ok(evidence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalog_is_valid_and_duplicate_topics_are_rejected() {
        let catalog = Catalog::parse(CATALOG_BYTES).expect("embedded catalog");
        let mut duplicate = serde_json::to_value(catalog).unwrap();
        let topics = duplicate["topics"].as_array_mut().unwrap();
        topics.push(topics[0].clone());
        assert!(Catalog::parse(&serde_json::to_vec(&duplicate).unwrap()).is_err());
    }
}
