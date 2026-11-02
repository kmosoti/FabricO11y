use super::evidence;
use std::{
    fs,
    path::Path,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
const SOURCE: &str = "aeneas-ab34bd2";

fn execute(mut command: Command) -> Result<String, String> {
    let debug = format!("{command:?}");
    let output = command.output().map_err(|e| format!("{debug}: {e}"))?;
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return Err(format!("{debug}: {}\n{report}", output.status));
    }
    Ok(report)
}

pub fn check(root: &Path) -> Result<(), String> {
    let source = evidence::tools().join(SOURCE);
    let charon = evidence::binary(
        "FABRIC_CHARON",
        &format!("{SOURCE}/charon/charon/target/release/charon"),
    );
    let aeneas = evidence::binary(
        "FABRIC_AENEAS",
        &format!("{SOURCE}/src/_build/default/main.exe"),
    );
    let lean = evidence::binary("FABRIC_LEAN", "lean-4.35.0-linux/bin/lean");
    let lake = lean.parent().ok_or("missing Lean directory")?.join("lake");
    for binary in [&charon, &aeneas, &lean, &lake] {
        println!("{} SHA256 {}", binary.display(), evidence::hash(binary)?);
    }
    if super::output(charon.to_str().ok_or("non-UTF8 path")?, &["version"], root)?
        != "0.1.304 (7b462d9159db2ed3015162352183906a30bfafc6)"
        || super::output(
            charon.to_str().ok_or("non-UTF8 path")?,
            &["toolchain-version"],
            root,
        )? != "nightly-2026-09-17"
    {
        return Err("unexpected Charon revision or extraction toolchain".into());
    }
    if !super::output(lean.to_str().ok_or("non-UTF8 path")?, &["--version"], root)?
        .starts_with("Lean (version 4.35.0,")
    {
        return Err("unexpected Lean version".into());
    }
    if !super::output(aeneas.to_str().ok_or("non-UTF8 path")?, &["-version"], root)?
        .contains("ab34bd28458a08c150b7cbf1873e59af74588138")
    {
        return Err("unexpected Aeneas source revision".into());
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let work = root.join(format!(".local/extraction-{stamp}"));
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    println!("Extraction artifacts: {}", work.display());
    let before = evidence::hash(&root.join("admission-kernel/src/lib.rs"))?;
    let properties = evidence::hash(&root.join("proofs/ExtractedAdmission.lean"))?;
    let result = (|| {
        let mut extract = Command::new("timeout");
        extract
            .arg("600")
            .arg(&charon)
            .args([
                "cargo",
                "--preset",
                "aeneas",
                "--sysroot",
                "default",
                "--error-on-warnings",
                "--dest-file",
            ])
            .arg(work.join("admission_kernel.llbc"))
            .args(["--", "--package", "admission-kernel", "--locked"])
            .env("RUSTUP_TOOLCHAIN", "nightly-2026-09-17")
            .env("CARGO_BUILD_JOBS", "2")
            .env("CARGO_TARGET_DIR", work.join("target"))
            .current_dir(root);
        fs::write(work.join("charon.log"), execute(extract)?).map_err(|e| e.to_string())?;
        let mut translate = Command::new("timeout");
        translate
            .arg("300")
            .arg(&aeneas)
            .args([
                "-backend",
                "lean",
                "-warnings-as-errors",
                "-abort-on-error",
                "-no-progress-bar",
                "-dest",
            ])
            .arg(&work)
            .arg(work.join("admission_kernel.llbc"))
            .current_dir(root);
        let report = execute(translate)?;
        fs::write(work.join("aeneas.log"), &report).map_err(|e| e.to_string())?;
        // An opaque application function is not covered by this proof path.
        let generated =
            fs::read_to_string(work.join("AdmissionKernel.lean")).map_err(|e| e.to_string())?;
        if generated.contains("axiom ")
            || generated.contains("sorry")
            || fs::read_dir(&work)
                .map_err(|e| e.to_string())?
                .any(|e| e.is_ok_and(|e| e.file_name().to_string_lossy().contains("External")))
        {
            return Err("opaque or incomplete extraction".into());
        }
        fs::copy(
            root.join("proofs/ExtractedAdmission.lean"),
            work.join("ExtractedAdmission.lean"),
        )
        .map_err(|e| e.to_string())?;
        let backend = source.join("backends/lean");
        let paths = super::output(
            lake.to_str().ok_or("non-UTF8 path")?,
            &["env", "printenv", "LEAN_PATH"],
            &backend,
        )?;
        let paths = std::env::join_paths(
            std::iter::once(work.clone()).chain(std::env::split_paths(&paths)),
        )
        .map_err(|e| e.to_string())?;
        for args in [
            vec!["-o", "AdmissionKernel.olean", "AdmissionKernel.lean"],
            vec!["-E", "warning", "ExtractedAdmission.lean"],
        ] {
            let mut check = Command::new("timeout");
            check
                .arg("300")
                .arg(&lean)
                .args(&args)
                .env("LEAN_PATH", &paths)
                .env("LEAN_NUM_THREADS", "2")
                .current_dir(&work);
            let report = execute(check)?;
            if report.contains("sorryAx") {
                return Err("incomplete Lean proof".into());
            }
            if args.last() == Some(&"ExtractedAdmission.lean") {
                for property in [
                    "reserve_preserves_bound",
                    "reserve_rejects_invalid_state",
                    "reserve_rejects_excess_charge",
                ] {
                    let expected = format!(
                        "'admission_kernel.{property}' depends on axioms: [propext, Classical.choice, Quot.sound]"
                    );
                    if !report.lines().any(|line| line == expected) {
                        return Err(format!("missing property or unexpected axioms: {property}"));
                    }
                }
            }
            println!("{report}");
            fs::write(work.join(format!("{}.log", args.last().unwrap())), report)
                .map_err(|e| e.to_string())?;
        }
        if before != evidence::hash(&root.join("admission-kernel/src/lib.rs"))? {
            return Err("kernel changed during extraction".into());
        }
        if properties != evidence::hash(&root.join("proofs/ExtractedAdmission.lean"))? {
            return Err("properties changed during checking".into());
        }
        Ok(())
    })();
    evidence::write(
        &work.join("result.json"),
        &serde_json::json!({"source_sha256": before, "properties_sha256": properties, "success": result.is_ok(), "error": result.as_ref().err()}),
    )?;
    result
}
