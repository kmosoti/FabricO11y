//! Checks over the evidence registries: counterexample fixtures and the
//! `cargo-mutants` equivalence list.

use crate::Violation;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

pub const MISSING_REPRODUCER: &str = "COUNTEREXAMPLE_MISSING_REPRODUCER";
pub const UNKNOWN_FIX: &str = "COUNTEREXAMPLE_UNKNOWN_FIX_COMMIT";
pub const INCOMPLETE_ENTRY: &str = "COUNTEREXAMPLE_INCOMPLETE_ENTRY";
pub const SURVIVED: &str = "MUTANT_SURVIVED";
pub const STALE_EQUIVALENT: &str = "MUTANT_STALE_EQUIVALENCE";

fn read(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Every counterexample names a reproducer test that exists in its file and
/// a fix commit that exists in the repository.
pub fn check_counterexamples(root: &Path, registry: &Path) -> Result<Vec<Violation>, String> {
    let json = read(registry)?;
    let entries = json
        .get("counterexamples")
        .and_then(Value::as_array)
        .ok_or("counterexample registry: missing `counterexamples`")?;
    let mut violations = Vec::new();
    for entry in entries {
        let field = |key: &str| entry.get(key).and_then(Value::as_str).unwrap_or("");
        let id = field("id");
        for key in [
            "id",
            "origin",
            "defect",
            "seed_or_trace",
            "contract",
            "fix_commit",
        ] {
            if field(key).is_empty() {
                violations.push(Violation::new(
                    INCOMPLETE_ENTRY,
                    id,
                    format!("missing `{key}`"),
                ));
            }
        }
        let file = entry
            .pointer("/reproducer/file")
            .and_then(Value::as_str)
            .unwrap_or("");
        let test = entry
            .pointer("/reproducer/test")
            .and_then(Value::as_str)
            .unwrap_or("");
        let text = std::fs::read_to_string(root.join(file)).unwrap_or_default();
        let defined = text.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with(&format!("fn {test}(")) || line.starts_with(&format!("def {test}("))
        });
        if test.is_empty() || !defined {
            violations.push(Violation::new(
                MISSING_REPRODUCER,
                id,
                format!("no test `{test}` in {file}"),
            ));
        }
        let commit = field("fix_commit");
        let known = Command::new("git")
            .args(["cat-file", "-e", &format!("{commit}^{{commit}}")])
            .current_dir(root)
            .status()
            .is_ok_and(|s| s.success());
        if !commit.is_empty() && !known {
            violations.push(Violation::new(
                UNKNOWN_FIX,
                id,
                format!("commit {commit} not found"),
            ));
        }
    }
    Ok(violations)
}

/// `path:line:col: description` without the position, which moves with edits.
fn without_position(line: &str) -> String {
    let mut parts = line.splitn(4, ':');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(file), Some(_), Some(_), Some(rest)) => format!("{file}:{rest}"),
        _ => line.to_owned(),
    }
}

/// Run cargo-mutants on core and app with both packages' tests, then compare
/// survivors and timeouts with the equivalence list.
pub fn cargo_mutants(root: &Path, equivalent: &Path) -> Result<Vec<Violation>, String> {
    let json = read(equivalent)?;
    let allowed: BTreeSet<String> = json
        .get("equivalent")
        .and_then(Value::as_array)
        .ok_or("equivalence list: missing `equivalent`")?
        .iter()
        .filter(|e| {
            e.get("reason")
                .and_then(Value::as_str)
                .is_some_and(|r| !r.is_empty())
        })
        .filter_map(|e| e.get("mutant").and_then(Value::as_str).map(str::to_owned))
        .collect();
    let out = root.join("target/cargo-mutants");
    let status = Command::new("cargo")
        .args([
            "mutants",
            "-p",
            "fabric-core",
            "-p",
            "fabric-app",
            "--test-package",
            "fabric-core",
            "--test-package",
            "fabric-app",
            "--jobs",
            "2",
            "-o",
        ])
        .arg(&out)
        .current_dir(root)
        .status()
        .map_err(|e| format!("cannot run cargo mutants: {e}"))?;
    // cargo-mutants exits 2 when mutants were missed and 3 on timeouts;
    // anything else non-zero means it could not run.
    if !matches!(status.code(), Some(0 | 2 | 3)) {
        return Err(format!("cargo mutants failed: {status}"));
    }
    let mut survivors = BTreeSet::new();
    for list in ["missed.txt", "timeout.txt"] {
        let text = std::fs::read_to_string(out.join("mutants.out").join(list)).unwrap_or_default();
        survivors.extend(text.lines().filter(|l| !l.is_empty()).map(without_position));
    }
    let mut violations = Vec::new();
    for survivor in &survivors {
        if !allowed.contains(survivor) {
            violations.push(Violation::new(
                SURVIVED,
                survivor,
                "not caught and not listed as equivalent",
            ));
        }
    }
    for entry in &allowed {
        if !survivors.contains(entry) {
            violations.push(Violation::new(
                STALE_EQUIVALENT,
                entry,
                "listed as equivalent but no longer survives",
            ));
        }
    }
    Ok(violations)
}
