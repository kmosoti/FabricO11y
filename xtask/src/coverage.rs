//! Declared requirement coverage is scoped test evidence, never completeness proof.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fs, path::Path, process::Command};
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    version: u32,
    invariants: Vec<Invariant>,
    checks: Vec<Check>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Invariant {
    id: String,
    requirement: String,
    required: bool,
    boundaries: Vec<String>,
    checks: Vec<String>,
    proof_scope: String,
    assumptions: Vec<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Check {
    id: String,
    package: String,
    target: String,
    test: String,
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}
impl Manifest {
    fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || self.invariants.is_empty()
            || self.checks.is_empty()
            || self.invariants.len() > 128
            || self.checks.len() > 128
        {
            return Err("coverage requires version 1 and 1..128 invariants/checks".into());
        }
        let mut ids = BTreeSet::new();
        let mut selectors = BTreeSet::new();
        for c in &self.checks {
            if !identifier(&c.id)
                || !ids.insert(&c.id)
                || !identifier(&c.package)
                || !identifier(&c.target)
                || c.test.is_empty()
                || c.test.len() > 256
                || !c
                    .test
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_:".contains(&b))
                || !selectors.insert((&c.package, &c.target, &c.test))
            {
                return Err("invalid or duplicate coverage check/selector".into());
            }
        }
        let mut invariants = BTreeSet::new();
        let mut used = BTreeSet::new();
        for i in &self.invariants {
            if !identifier(&i.id)
                || !invariants.insert(&i.id)
                || i.requirement.trim().is_empty()
                || i.boundaries.len() < 2
                || i.boundaries.iter().any(|s| s.trim().is_empty())
                || i.proof_scope.trim().is_empty()
                || i.assumptions.is_empty()
                || i.assumptions.iter().any(|s| s.trim().is_empty())
                || (i.required && i.checks.is_empty())
            {
                return Err("invalid invariant or unresolved required coverage".into());
            }
            let mut local = BTreeSet::new();
            for check in &i.checks {
                if !ids.contains(check) || !local.insert(check) {
                    return Err("missing or duplicate invariant check reference".into());
                }
                used.insert(check);
            }
        }
        if used.len() != ids.len() {
            return Err("unreferenced coverage check".into());
        }
        Ok(())
    }
}
fn load(root: &Path) -> Result<Manifest, String> {
    let bytes = fs::read(root.join("verification/coverage.toml")).map_err(|e| e.to_string())?;
    if bytes.len() > 256 * 1024 {
        return Err("coverage manifest exceeds 256 KiB".into());
    }
    let m: Manifest = toml::from_str(std::str::from_utf8(&bytes).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    m.validate()?;
    Ok(m)
}
pub fn validate(root: &Path) -> Result<(), String> {
    load(root).map(|_| ())
}
pub fn digest(root: &Path) -> Result<String, String> {
    super::evidence::hash(&root.join("verification/coverage.toml"))
}
fn passed_exact_test(stdout: &str, name: &str) -> bool {
    stdout
        .lines()
        .filter(|s| *s == format!("test {name} ... ok"))
        .count()
        == 1
        && stdout
            .lines()
            .filter(|s| s.starts_with("test result:"))
            .count()
            == 1
        && stdout
            .lines()
            .any(|s| s.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored;"))
}
#[derive(Serialize)]
struct Execution {
    id: String,
    test: String,
    status: &'static str,
}
pub fn run(root: &Path) -> Result<(), String> {
    let report_path = root.join(".local/coverage.json");
    match fs::remove_file(&report_path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.to_string()),
    }
    let manifest = load(root)?;
    let digest = digest(root)?;
    let mut results = Vec::new();
    for check in &manifest.checks {
        let mut command = Command::new("timeout");
        command
            .args([
                "120",
                "cargo",
                "test",
                "--locked",
                "-p",
                &check.package,
                "--test",
                &check.target,
                &check.test,
                "--",
                "--exact",
                "--test-threads=1",
            ])
            .env("CARGO_TERM_COLOR", "never")
            .env("RUST_TEST_NOCAPTURE", "0")
            .current_dir(root);
        let original = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        if root != original {
            command.env(
                "CARGO_TARGET_DIR",
                root.parent()
                    .ok_or("source parent")?
                    .join("candidate-build"),
            );
        }
        let output = command.output().map_err(|e| e.to_string())?;
        if !output.status.success()
            || !passed_exact_test(&String::from_utf8_lossy(&output.stdout), &check.test)
        {
            return Err(format!(
                "coverage check {} did not execute exactly one passing, non-ignored test: {}\n{}\n{}",
                check.id,
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        results.push(Execution {
            id: check.id.clone(),
            test: check.test.clone(),
            status: "tested",
        });
    }
    if digest != self::digest(root)? {
        return Err("coverage manifest changed during execution".into());
    }
    let invariants: Vec<_> = manifest
        .invariants
        .iter()
        .map(|i| {
            serde_json::json!({
                "id": i.id,
                "required": i.required,
                "status": if i.checks.is_empty() { "uncovered" } else { "tested" },
                "boundaries": i.boundaries,
                "checks": i.checks,
                "proof_scope": i.proof_scope,
                "assumptions": i.assumptions,
            })
        })
        .collect();
    fs::create_dir_all(root.join(".local")).map_err(|e| e.to_string())?;
    let source = super::output(
        "jj",
        &[
            "--ignore-working-copy",
            "log",
            "--no-graph",
            "-r",
            "@",
            "-T",
            "commit_id",
        ],
        root,
    )?;
    let report = serde_json::json!({
        "schema": 1,
        "source_snapshot": source,
        "source_binding": "candidate verification receipt establishes source binding; standalone execution does not establish working-file freshness",
        "manifest_sha256": digest,
        "scope": "named tests executed; declared mapping is not completeness or whole-system proof",
        "checks": results,
        "invariants": invariants,
    });
    fs::write(
        report_path,
        serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    println!(
        "Declared coverage: {} exact tests passed; manifest {digest}",
        results.len()
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn valid() -> Manifest {
        Manifest {
            version: 1,
            invariants: vec![Invariant {
                id: "readback".into(),
                requirement: "acknowledged is readable".into(),
                required: true,
                boundaries: vec!["admission".into(), "read".into()],
                checks: vec!["lifecycle".into()],
                proof_scope: "test evidence only".into(),
                assumptions: vec!["storage honors sync".into()],
            }],
            checks: vec![Check {
                id: "lifecycle".into(),
                package: "fabric".into(),
                target: "readback_lifecycle".into(),
                test: "lifecycle".into(),
            }],
        }
    }
    #[test]
    fn refuses_missing_duplicate_and_unresolved_bindings() {
        let m = valid();
        assert!(m.validate().is_ok());
        let mut bad = m.clone();
        bad.checks.push(bad.checks[0].clone());
        assert!(bad.validate().is_err());
        let mut bad = m.clone();
        bad.invariants[0].checks = vec!["absent".into()];
        assert!(bad.validate().is_err());
        let mut bad = m.clone();
        bad.invariants[0].checks.clear();
        assert!(bad.validate().is_err());
        let mut bad = m.clone();
        bad.invariants[0].checks.push("lifecycle".into());
        assert!(bad.validate().is_err());
        let mut bad = m;
        bad.invariants.push(bad.invariants[0].clone());
        assert!(bad.validate().is_err());
    }
    #[test]
    fn broad_success_zero_matches_and_ignored_do_not_cover() {
        assert!(passed_exact_test(
            "test lifecycle ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 2 filtered out; finished in 0.1s\n",
            "lifecycle"
        ));
        for output in [
            "test result: ok. 0 passed; 0 failed; 0 ignored;",
            "test lifecycle ... ignored\ntest result: ok. 0 passed; 0 failed; 1 ignored;",
            "test other ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;",
            "test lifecycle ... ok\ntest other ... ok\ntest result: ok. 2 passed; 0 failed; 0 ignored;",
        ] {
            assert!(!passed_exact_test(output, "lifecycle"));
        }
    }
    #[test]
    fn hook_distinguishes_snapshot_coverage_and_unchecked_working_files() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for p in [".codex/hooks", ".local", "verification", "bin"] {
            fs::create_dir_all(root.join(p)).unwrap();
        }
        fs::write(
            root.join(".codex/hooks/context.sh"),
            include_str!("../../.codex/hooks/context.sh"),
        )
        .unwrap();
        fs::write(root.join("verification/coverage.toml"), "manifest").unwrap();
        fs::write(
            root.join("bin/jj"),
            "#!/bin/sh\nprintf '%s' aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
        )
        .unwrap();
        fs::set_permissions(root.join("bin/jj"), fs::Permissions::from_mode(0o755)).unwrap();
        let run = || {
            let output = Command::new("sh")
                .arg(root.join(".codex/hooks/context.sh"))
                .env(
                    "PATH",
                    format!(
                        "{}:{}",
                        root.join("bin").display(),
                        std::env::var("PATH").unwrap()
                    ),
                )
                .output()
                .unwrap();
            assert!(output.status.success());
            serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["systemMessage"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert!(run().contains("No local verification receipt"));
        let receipt = format!(
            "candidate={}\ncoverage_manifest={}\ncoverage=declared-required-tested\nresult=success\n",
            "a".repeat(40),
            super::digest(root).unwrap()
        );
        fs::write(root.join(".local/last-verification"), &receipt).unwrap();
        assert!(run().contains("declared required tests passed"));
        assert!(run().contains("freshness is unchecked"));
        fs::write(root.join("verification/coverage.toml"), "changed").unwrap();
        assert!(run().contains("manifest changed"));
        fs::write(
            root.join(".local/last-verification"),
            receipt.replace("result=success", "result=failed: test failed"),
        )
        .unwrap();
        assert!(run().contains("unsuccessful"));
        fs::write(
            root.join(".local/last-verification"),
            receipt.replace(&"a".repeat(40), &"b".repeat(40)),
        )
        .unwrap();
        assert!(run().contains("stale"));
    }
}
