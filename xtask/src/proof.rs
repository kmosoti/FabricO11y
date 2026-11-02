use super::evidence;
use std::{path::Path, process::Command};
const VERUS: &str = "verus-0.2026.10.26.d17fbd0/verus-x86-linux";

pub fn check(which: &str, root: &Path) -> Result<(), String> {
    match which {
        "selftest" => negative_controls(root),
        "verus" => {
            let verus = evidence::binary("FABRIC_VERUS", &format!("{VERUS}/verus"));
            let parent = verus.parent().ok_or("invalid Verus path")?;
            let version =
                super::output(verus.to_str().ok_or("non-UTF8 path")?, &["--version"], root)?;
            if !version.contains("Version: 0.2026.10.26.d17fbd0") {
                return Err("unexpected Verus version".into());
            }
            println!("Verus SHA256: {}", evidence::hash(&verus)?);
            let z3 = parent.join("z3");
            if super::output(z3.to_str().ok_or("non-UTF8 path")?, &["--version"], root)?
                != "Z3 version 4.16.0 - 64 bit"
            {
                return Err("unexpected Z3 version".into());
            }
            println!("Z3 SHA256: {}", evidence::hash(&z3)?);
            let result = Command::new("timeout")
                .arg("600")
                .arg(parent.join("cargo-verus"))
                .args([
                    "verus",
                    "verify",
                    "-p",
                    "admission-kernel",
                    "--fwd-verus-args-to",
                    "roots",
                    "--locked",
                    "--",
                    "--no-cheating",
                    "--num-threads",
                    "2",
                ])
                .env("VERUS_Z3_PATH", &z3)
                .current_dir(root)
                .status()
                .map_err(|e| e.to_string())?;
            if !result.success() {
                return Err(format!("Verus failed: {result}"));
            }
            Ok(())
        }
        "lean" => {
            let lean = evidence::binary("FABRIC_LEAN", "lean-4.35.0-linux/bin/lean");
            let lean = lean.to_str().ok_or("non-UTF8 path")?;
            if !super::output(lean, &["--version"], root)?.starts_with("Lean (version 4.35.0,") {
                return Err("unexpected Lean version".into());
            }
            let report = super::output(lean, &["-E", "warning", "proofs/Admission.lean"], root)?;
            let expected = "'Admission.reserve_preserves_bound' depends on axioms: [propext, Quot.sound]\n'Admission.reserve_rejects_overflow' depends on axioms: [propext, Quot.sound]";
            if report != expected {
                return Err(format!("unexpected axiom dependencies: {report}"));
            }
            println!("{report}");
            Ok(())
        }
        "aeneas" => super::extraction::check(root),
        "collection" => collection(root),
        "all" => {
            for checker in ["verus", "lean", "collection", "aeneas"] {
                check(checker, root)?;
            }
            Ok(())
        }
        _ => Err("expected verus, aeneas, lean, collection, or all".into()),
    }
}

fn negative_controls(root: &Path) -> Result<(), String> {
    let work = tempfile::tempdir().map_err(|e| e.to_string())?;
    let verus = evidence::binary("FABRIC_VERUS", &format!("{VERUS}/verus"));
    let lean = evidence::binary("FABRIC_LEAN", "lean-4.35.0-linux/bin/lean");
    let cases = [
        (
            &verus,
            "false.rs",
            "use vstd::prelude::*; verus! { pub fn invalid_transition() ensures false {} }",
            "use vstd::prelude::*; verus! { pub fn valid_transition() ensures true {} }",
            vec![
                "--crate-type",
                "lib",
                "--crate-name",
                "checker_control",
                "--no-cheating",
                "--num-threads",
                "2",
            ],
        ),
        (
            &lean,
            "False.lean",
            "example : False := by decide",
            "example : True := by trivial",
            vec!["-E", "warning"],
        ),
    ];
    for (checker, name, source, positive, arguments) in cases {
        let file = work.path().join(name);
        std::fs::write(&file, positive).map_err(|e| e.to_string())?;
        let output = Command::new("timeout")
            .arg("120")
            .arg(checker)
            .args(&arguments)
            .arg(&file)
            .current_dir(root)
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!(
                "positive checker control failed: {} {}\n{}{}",
                checker.display(),
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            ));
        }
        std::fs::write(&file, source).map_err(|e| e.to_string())?;
        let output = Command::new("timeout")
            .arg("120")
            .arg(checker)
            .args(arguments)
            .arg(&file)
            .current_dir(root)
            .output()
            .map_err(|e| e.to_string())?;
        if output.status.success() || output.status.code() == Some(124) {
            return Err(format!(
                "negative control did not establish rejection: {} {}",
                checker.display(),
                output.status
            ));
        }
        let report = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if !report.contains("error") {
            return Err("negative control failed without a checker diagnostic".into());
        }
        println!("{name}: false property rejected ({})", output.status);
    }
    let missing = work.path().join("missing-checker");
    if Command::new(missing).status().is_ok() {
        return Err("missing-tool negative control failed".into());
    }
    println!("Missing checker refused");
    Ok(())
}

fn collection(root: &Path) -> Result<(), String> {
    let lean = evidence::binary("FABRIC_LEAN", "lean-4.35.0-linux/bin/lean");
    let executable = lean.to_str().ok_or("non-UTF8 path")?;
    if !super::output(executable, &["--version"], root)?.starts_with("Lean (version 4.35.0,") {
        return Err("unexpected Lean version".into());
    }
    let report = super::output(
        executable,
        &["-E", "warning", "proofs/Collection.lean"],
        root,
    )?;
    for property in [
        "admission_preserves_bound",
        "full_requires_refusal",
        "confirmation_preserves_bound",
        "fixed_frontier_stays_covered",
        "retained_bytes_bound",
        "full_payload_attains_bound",
        "full_records_cost",
        "lossless_byte_storage_lower_bound",
        "duplicate_preserves_state",
        "conflict_preserves_state",
        "abort_preserves_state",
        "commit_publishes_both",
        "receipt_implies_retention",
    ] {
        let prefix = format!("'Collection.{property}' ");
        let valid = report
            .lines()
            .find_map(|l| l.strip_prefix(&prefix))
            .is_some_and(|s| {
                s == "does not depend on any axioms"
                    || s.strip_prefix("depends on axioms: [")
                        .and_then(|s| s.strip_suffix(']'))
                        .is_some_and(|s| {
                            !s.is_empty()
                                && s.split(", ").all(|a| {
                                    ["propext", "Classical.choice", "Quot.sound"].contains(&a)
                                })
                        })
            });
        if !valid {
            return Err(format!(
                "missing or untrusted collection theorem: {property}"
            ));
        }
    }
    println!("{report}");
    // A skipped acknowledgement must fail the executable contract, not just a unit test.
    let original = std::fs::read_to_string(root.join("admission-kernel/src/delivery.rs"))
        .map_err(|e| e.to_string())?;
    let mutant = original.replace("receipt == confirmed + 1", "receipt > confirmed");
    if mutant == original {
        return Err("delivery mutation no longer matches".into());
    }
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let file = dir.path().join("delivery_mutant.rs");
    std::fs::write(&file, mutant).map_err(|e| e.to_string())?;
    let verus = evidence::binary("FABRIC_VERUS", &format!("{VERUS}/verus"));
    let output = Command::new("timeout")
        .arg("120")
        .arg(verus)
        .args(["--crate-type", "lib", "--no-cheating", "--num-threads", "2"])
        .arg(file)
        .output()
        .map_err(|e| e.to_string())?;
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success()
        || output.status.code() == Some(124)
        || !diagnostics.contains("postcondition not satisfied")
    {
        return Err(format!(
            "skipped-receipt mutant not rejected by proof: {diagnostics}"
        ));
    }
    println!("Skipped-receipt mutant rejected by Verus postcondition");
    Ok(())
}
