//! Repository checks that read Cargo metadata. Each check returns violations
//! with a stable category code so tests can assert *why* a fixture failed,
//! not merely that a command exited non-zero.

pub mod checks;
pub mod layers;
pub mod metadata;
pub mod mutants;
pub mod purity;

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Violation {
    /// Stable category, e.g. `LAYER_FORBIDDEN_EDGE`.
    pub code: &'static str,
    pub subject: String,
    pub detail: String,
}

impl Violation {
    pub fn new(code: &'static str, subject: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code,
            subject: subject.into(),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}: {}", self.code, self.subject, self.detail)
    }
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: invalid JSON: {e}", path.display()))
}

/// Run the layer gate. `Err` means the check could not run, which is never a pass.
pub fn check_layers(
    manifest: &Path,
    policy: &Path,
    options: metadata::Options,
) -> Result<Vec<Violation>, String> {
    let policy = layers::Policy::parse(&read_json(policy)?)?;
    let metadata = metadata::load(manifest, options)?;
    let mut violations = layers::check(&metadata, &policy);
    violations.sort();
    violations.dedup();
    Ok(violations)
}

/// Run the core-purity gate. `Err` means the check could not run.
pub fn check_core_purity(
    manifest: &Path,
    policy: &Path,
    options: metadata::Options,
) -> Result<Vec<Violation>, String> {
    let policy = purity::Policy::parse(&read_json(policy)?)?;
    let metadata = metadata::load(manifest, options)?;
    let mut violations = purity::check(&metadata, &policy);
    violations.sort();
    violations.dedup();
    Ok(violations)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
