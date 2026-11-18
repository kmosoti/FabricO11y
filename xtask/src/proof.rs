use super::evidence;
use std::{path::Path, process::Command};
const VERUS: &str = "verus-0.2026.10.26.d17fbd0/verus-x86-linux";

fn verus_binary() -> std::path::PathBuf {
    evidence::binary("FABRIC_VERUS", &format!("{VERUS}/verus"))
}

pub(super) fn lean_binary() -> std::path::PathBuf {
    evidence::binary("FABRIC_LEAN", "lean-4.35.0-linux/bin/lean")
}

pub(super) fn checker_binaries() -> Result<[(&'static str, std::path::PathBuf); 5], String> {
    let verus = verus_binary();
    let parent = verus.parent().ok_or("invalid Verus path")?;
    let z3 = parent.join("z3");
    let cargo_verus = parent.join("cargo-verus");
    let lean = lean_binary();
    let lake = lean.parent().ok_or("missing Lean directory")?.join("lake");
    Ok([
        ("lean", lean),
        ("lake", lake),
        ("verus", verus),
        ("z3", z3),
        ("cargo-verus", cargo_verus),
    ])
}

pub(super) fn guide_checker(checker: &str, path: &str) -> bool {
    matches!(
        (checker, path),
        ("aeneas", "proofs/ExtractedAdmission.lean")
            | ("lean", "proofs/Admission.lean")
            | ("custody", "proofs/Custody.lean")
            | ("collection", "proofs/Collection.lean")
            | ("distributed", "proofs/Distributed.lean")
            | ("telemetry", "proofs/Telemetry.lean")
    )
}

pub fn check(which: &str, root: &Path) -> Result<(), String> {
    match which {
        "selftest" => negative_controls(root),
        "verus" => {
            let verus = verus_binary();
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
            let lean = lean_binary();
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
        "distributed" => distributed(root),
        "custody" => custody(root),
        "smt" => smt(root),
        "telemetry" => telemetry(root),
        "all" => {
            for checker in [
                "verus",
                "lean",
                "collection",
                "distributed",
                "custody",
                "smt",
                "telemetry",
                "aeneas",
            ] {
                check(checker, root)?;
            }
            Ok(())
        }
        _ => Err("expected verus, aeneas, lean, collection, distributed, custody, smt, telemetry, or all".into()),
    }
}

fn negative_controls(root: &Path) -> Result<(), String> {
    let work = evidence::non_tmp_tempdir()?;
    let verus = verus_binary();
    let lean = lean_binary();
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
    let frontier_source = std::fs::read_to_string(root.join("admission-kernel/src/telemetry.rs"))
        .map_err(|e| e.to_string())?;
    let commit_gate = "if origin_matches && flush_succeeded && first > committed";
    if frontier_source.matches(commit_gate).count() != 1 {
        return Err("projection frontier commit gate mutation no longer matches".into());
    }
    let mutant = work.path().join("frontier-mutant.rs");
    std::fs::write(
        &mutant,
        frontier_source.replace(commit_gate, "if origin_matches && first > committed"),
    )
    .map_err(|e| e.to_string())?;
    let output = Command::new("timeout")
        .arg("120")
        .arg(&verus)
        .args([
            "--crate-type",
            "lib",
            "--crate-name",
            "projection_frontier_control",
            "--no-cheating",
            "--num-threads",
            "2",
        ])
        .arg(&mutant)
        .current_dir(work.path())
        .output()
        .map_err(|e| e.to_string())?;
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    if output.status.code() != Some(1)
        || !diagnostics.contains("postcondition not satisfied")
        || !diagnostics.contains("frontier-mutant.rs")
    {
        return Err(format!(
            "projection frontier partial-publication mutant not rejected by contract: {diagnostics}"
        ));
    }
    println!("Projection frontier partial-publication mutant rejected by Verus postcondition");
    let missing = work.path().join("missing-checker");
    if Command::new(missing).status().is_ok() {
        return Err("missing-tool negative control failed".into());
    }
    println!("Missing checker refused");
    Ok(())
}

fn collection(root: &Path) -> Result<(), String> {
    let lean = lean_binary();
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
    let dir = evidence::non_tmp_tempdir()?;
    let file = dir.path().join("delivery_mutant.rs");
    std::fs::write(&file, mutant).map_err(|e| e.to_string())?;
    let verus = verus_binary();
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

fn distributed(root: &Path) -> Result<(), String> {
    let lean = lean_binary();
    let executable = lean.to_str().ok_or("non-UTF8 checker")?;
    if !super::output(executable, &["--version"], root)?.starts_with("Lean (version 4.35.0,") {
        return Err("unexpected Lean".into());
    }
    let report = super::output(
        executable,
        &["-E", "warning", "proofs/Distributed.lean"],
        root,
    )?;
    for name in [
        "publish_preserves_existing",
        "step_preserves_remote",
        "arbitrary_trace_retains_remote",
        "lost_ack_can_be_durable",
        "unknown_ack_does_not_advance",
        "local_is_not_remote",
        "publish_retains",
        "conflict_cannot_overwrite",
        "duplicates_idempotent",
        "other_origin_unchanged",
        "local_custody_survives_handoff",
        "delivered_empty_server_seals",
        "frontier_cannot_skip",
        "frontier_advance",
        "reclamation_requires_remote",
        "bounded_enqueue",
        "full_refuses",
        "finite_drain",
    ] {
        let prefix = format!("'Distributed.{name}' ");
        let allowed = report
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
        if !allowed {
            return Err(format!("missing or untrusted distributed theorem: {name}"));
        }
    }
    let original =
        std::fs::read_to_string(root.join("proofs/Distributed.lean")).map_err(|e| e.to_string())?;
    let mutant = original.replace(
        "| some _ => store",
        "| some _ => fun j => if j = k then some p else store j",
    );
    if original == mutant {
        return Err("conflict mutation no longer matches".into());
    }
    let temp = evidence::non_tmp_tempdir()?;
    let file = temp.path().join("Mutant.lean");
    std::fs::write(&file, mutant).map_err(|e| e.to_string())?;
    let output = Command::new("timeout")
        .arg("60")
        .arg(&lean)
        .args(["-E", "warning"])
        .arg(file)
        .output()
        .map_err(|e| e.to_string())?;
    let diagnostic = String::from_utf8_lossy(&output.stdout);
    if output.status.success()
        || output.status.code() == Some(124)
        || !diagnostic.contains("unsolved goals")
    {
        return Err(format!(
            "conflict mutant did not fail its theorem: {diagnostic}"
        ));
    }
    println!("{report}\nConflict-overwrite mutant rejected");
    Ok(())
}

fn custody(root: &Path) -> Result<(), String> {
    let lean = lean_binary();
    let executable = lean.to_str().ok_or("non-UTF8 checker")?;
    if !super::output(executable, &["--version"], root)?.starts_with("Lean (version 4.35.0,") {
        return Err("unexpected Lean".into());
    }
    let report = super::output(executable, &["-E", "warning", "proofs/Custody.lean"], root)?;
    let names = [
        "retry_identity_cannot_move_to_new_source",
        "publish_keeps_binding",
        "arbitrary_initial_trace_has_no_holes_or_unallocated_ids",
        "reserve_assigns_fresh_record",
        "engine_write_other_id",
        "delayed_exact_retry_preserves_new_bytes",
        "reservation_preserves_allocation_bound",
        "step_preserves_allocation_bound",
        "trace_preserves_allocation_bound",
        "step_preserves_prefix",
        "arbitrary_trace_preserves_source_prefix",
        "existing_binding_immutable",
        "conflicting_retry_refused",
        "reservation_precedes_upsert",
        "unknown_retains_reservation_and_ids",
        "fresh_id_separates_old",
        "delayed_packet_preserves_fresh_id",
        "missing_readback_blocks_publication",
        "publication_requires_full_readback",
        "publication_never_skips_source",
        "step_preserves_binding",
        "arbitrary_trace_preserves_binding",
        "shadow_cannot_delete",
        "gc_requires_matching_confirmation",
        "expiry_independent_of_metadata",
        "unauthenticated_lease_refused",
        "legacy_not_retroactively_expired",
        "retirement_requires_expiry_and_forwarding",
        "pending_generation_survives_expiry",
        "cancellation_retains_credit_until_cleanup",
    ];
    if report.lines().count() != names.len() {
        return Err("unexpected custody axiom audit count".into());
    }
    for name in names {
        let prefix = format!("'Custody.{name}' ");
        let entries = report
            .lines()
            .filter_map(|line| line.strip_prefix(&prefix))
            .collect::<Vec<_>>();
        if entries.len() != 1
            || !(entries[0] == "does not depend on any axioms"
                || entries[0]
                    .strip_prefix("depends on axioms: [")
                    .and_then(|s| s.strip_suffix(']'))
                    .is_some_and(|s| {
                        !s.is_empty()
                            && s.split(", ")
                                .all(|a| ["propext", "Quot.sound"].contains(&a))
                    }))
        {
            return Err(format!("missing or untrusted custody theorem: {name}"));
        }
    }
    // Independent concrete witnesses distinguish a semantic rejection from a
    // broken checker, malformed mutation, timeout or unused-variable warning.
    let original =
        std::fs::read_to_string(root.join("proofs/Custody.lean")).map_err(|e| e.to_string())?;
    let definitions = original
        .split_once("theorem existing_binding_immutable")
        .ok_or("custody mutation boundary moved")?
        .0;
    let controls = r#"
def controlRecord : Record := ⟨1,7,[1]⟩
def controlState : State :=
  {binding := fun key => if key = (0,1) then some controlRecord else none
   owner := fun key => if key = (0,7) then some (0,1) else none
   pending := fun _ => true
   engine := fun _ => none
   visible := fun _ => false
   frontier := fun _ => 0
   allocated := 1}
theorem conflicting_reservation_control :
    (reserve controlState (0,1) 7 [2]).binding (0,1) = some controlRecord := by decide
theorem readback_before_publish_control :
    (publish controlState (0,1)).visible (0,1) = false := by decide
end Custody
"#;
    let positive = format!("{definitions}{controls}");
    let temp = evidence::non_tmp_tempdir()?;
    let file = temp.path().join("Controls.lean");
    std::fs::write(&file, &positive).map_err(|e| e.to_string())?;
    super::output(
        executable,
        &["-E", "warning", file.to_str().ok_or("control path")?],
        root,
    )?;
    let conflict = "  | some _ => s\n";
    let readback = " ∧ s.engine r.id = some r";
    if positive.matches(conflict).count() != 1 || positive.matches(readback).count() != 1 {
        return Err("custody semantic mutation no longer matches exactly".into());
    }
    let mutants=[
        positive.replace(conflict,"  | some old => {s with binding := fun k => if k = key then some {old with identity := identity, bytes := bytes} else s.binding k}\n"),
        positive.replace(readback,"").replace("  | some r =>\n    if s.pending key","  | some _r =>\n    if s.pending key"),
    ];
    for (index, mutant) in mutants.iter().enumerate() {
        let file = temp.path().join(format!("Mutant{index}.lean"));
        std::fs::write(&file, mutant).map_err(|e| e.to_string())?;
        let output = Command::new("timeout")
            .arg("60")
            .arg(&lean)
            .args(["-E", "warning"])
            .arg(file)
            .current_dir(root)
            .output()
            .map_err(|e| e.to_string())?;
        let diagnostic = String::from_utf8_lossy(&output.stdout);
        if output.status.success()
            || output.status.code() == Some(124)
            || !diagnostic.contains("decide")
            || !diagnostic.contains("is false")
        {
            return Err(format!(
                "custody semantic mutant {index} did not fail its witness: {diagnostic}"
            ));
        }
    }
    println!("{report}\nConflicting-reservation and missing-readback mutants rejected");
    Ok(())
}

fn smt(root: &Path) -> Result<(), String> {
    let verus = verus_binary();
    let parent = verus.parent().ok_or("invalid Verus path")?;
    let z3 = parent.join("z3");
    let z3_str = z3.to_str().ok_or("non-UTF8 Z3 path")?;
    if super::output(z3_str, &["--version"], root)? != "Z3 version 4.16.0 - 64 bit" {
        return Err("unexpected Z3 version".into());
    }
    println!("Z3 SHA256: {}", evidence::hash(&z3)?);
    let report = super::output(z3_str, &["-smt2", "proofs/TelemetrySmt.smt2"], root)?;
    let results: Vec<&str> = report
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if results != ["unsat", "unsat", "unsat", "unsat"] {
        return Err(format!("unexpected Z3 SMT output: {report}"));
    }
    let original = std::fs::read_to_string(root.join("proofs/TelemetrySmt.smt2"))
        .map_err(|e| e.to_string())?;
    let needle = "(define-fun enc_d1 () (_ BitVec 64) (bvsub t1 t0))";
    if original.matches(needle).count() != 1 {
        return Err("SMT Gorilla mutant boundary no longer matches".into());
    }
    let mutant = original.replace(
        needle,
        "(define-fun enc_d1 () (_ BitVec 64) (ite (bvuge t1 t0) (bvsub t1 t0) #x0000000000000000))",
    );
    let temp = evidence::non_tmp_tempdir()?;
    let file = temp.path().join("TelemetrySmtMutant.smt2");
    std::fs::write(&file, mutant).map_err(|e| e.to_string())?;
    let mutant_out = Command::new("timeout")
        .arg("60")
        .arg(&z3)
        .args(["-smt2"])
        .arg(&file)
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&mutant_out.stdout);
    if !mutant_out.status.success() || !stdout.lines().any(|line| line.trim() == "sat") {
        return Err(format!("SMT Gorilla mutant did not yield sat: {stdout}"));
    }
    println!(
        "TelemetrySmt.smt2: 4/4 QF_BV properties unsat; saturating-delta mutant rejected (sat)"
    );
    Ok(())
}

fn telemetry(root: &Path) -> Result<(), String> {
    let lean = lean_binary();
    let executable = lean.to_str().ok_or("non-UTF8 checker")?;
    if !super::output(executable, &["--version"], root)?.starts_with("Lean (version 4.35.0,") {
        return Err("unexpected Lean version".into());
    }
    let report = super::output(
        executable,
        &["-E", "warning", "proofs/Telemetry.lean"],
        root,
    )?;
    let names = [
        "gorilla_step_roundtrip",
        "gorilla_stream_roundtrip",
        "rollup_bucket_aligned",
        "rollup_bucket_residual_bounds",
        "rollup_bucket_idempotent",
        "rollup_bucket_monotone",
        "watermark_failed_flush_unchanged",
        "watermark_step_monotone",
        "watermark_trace_monotone",
    ];
    if report.lines().count() != names.len() {
        return Err(format!("unexpected Telemetry axiom audit count: {report}"));
    }
    for name in names {
        let prefix = format!("'Telemetry.{name}' ");
        let entries: Vec<&str> = report
            .lines()
            .filter_map(|line| line.strip_prefix(&prefix))
            .collect();
        if entries.len() != 1
            || !(entries[0] == "does not depend on any axioms"
                || entries[0]
                    .strip_prefix("depends on axioms: [")
                    .and_then(|s| s.strip_suffix(']'))
                    .is_some_and(|s| {
                        !s.is_empty()
                            && s.split(", ").all(|axiom| {
                                ["propext", "Classical.choice", "Quot.sound"].contains(&axiom)
                            })
                    }))
        {
            return Err(format!("missing or untrusted telemetry theorem: {name}"));
        }
    }
    let original =
        std::fs::read_to_string(root.join("proofs/Telemetry.lean")).map_err(|e| e.to_string())?;
    let needle = "  if flushOk && decide (candidate > committed) then candidate else committed";
    if original.matches(needle).count() != 1 {
        return Err("Telemetry Lean mutant boundary no longer matches".into());
    }
    let mutant = original.replace(needle, "  candidate");
    let temp = evidence::non_tmp_tempdir()?;
    let file = temp.path().join("TelemetryMutant.lean");
    std::fs::write(&file, mutant).map_err(|e| e.to_string())?;
    let output = Command::new("timeout")
        .arg("60")
        .arg(&lean)
        .args(["-E", "warning"])
        .arg(&file)
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() || output.status.code() == Some(124) || !diagnostic.contains("error")
    {
        return Err(format!(
            "telemetry failed-flush mutant was not rejected: {diagnostic}"
        ));
    }
    println!("{report}\nTelemetry failed-flush mutant rejected");
    Ok(())
}
