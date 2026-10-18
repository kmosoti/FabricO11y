//! Baseline-only experiment: synthetic receipts exercised through the original
//! Python CLI. This driver deliberately contains no candidate verifier.

use serde_json::{Map, Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git").args(args).current_dir(root).output().map_err(|e| e.to_string())?;
    if !output.status.success() { return Err(format!("git {:?} failed", args)); }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn write_json_new(path: &Path, value: &Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    write_new(path, &bytes)
}

fn create_fixture(registry: &Path, receipts: &Path, commit: &str) -> Result<Vec<String>, String> {
    fs::create_dir(receipts).map_err(|e| e.to_string())?;
    let checks = xtask::checks::load(registry)?;
    let selected: Vec<_> = checks.into_iter().filter(|c| c.profile == "extended").collect();
    if selected.is_empty() { return Err("registry has no extended checks".into()); }
    let mut ids = Vec::new();
    for check in selected {
        let id = check.id.clone();
        let log_name = format!("{id}.log");
        let log = format!("SYNTHETIC RECEIPT FIXTURE; no command executed; check={id}\n").into_bytes();
        write_new(&receipts.join(&log_name), &log)?;
        let command: Vec<_> = check.command.iter().cloned().map(Value::String).collect();
        let receipt = json!({
            "receipt_version": 1,
            "check_id": id,
            "property_ids": check.raw.get("contracts").cloned().unwrap_or(Value::Null),
            "candidate_commit": commit,
            "worktree_dirty": false,
            "specification_revision": "synthetic",
            "oracle_revision": "synthetic",
            "toolchain": {},
            "command": command,
            "profile": "extended",
            "scope": check.raw.get("scope").cloned().unwrap_or(Value::Null),
            "fixtures": check.raw.get("fixtures").cloned().unwrap_or(Value::Null),
            "bounds": check.raw.get("bounds").cloned().unwrap_or(Value::Null),
            "negative_controls": check.raw.get("negative_controls").cloned().unwrap_or(Value::Null),
            "expected": check.raw.get("expected").cloned().unwrap_or(Value::Null),
            "result": "passed",
            "exit_code": 0,
            "duration_ms": 1,
            "output_log": log_name,
            "output_sha256": xtask::sha256_hex(&log),
            "unchecked_behavior": check.raw.get("unchecked").cloned().unwrap_or(Value::Null),
            "trust": "synthetic fixture; does not attest command execution"
        });
        write_json_new(&receipts.join(format!("{id}.json")), &receipt)?;
        ids.push(id);
    }
    Ok(ids)
}

fn edit_receipt(receipts: &Path, id: &str, change: impl FnOnce(&mut Map<String, Value>)) -> Result<(), String> {
    let path = receipts.join(format!("{id}.json"));
    let mut value: Value = serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let object = value.as_object_mut().ok_or("fixture receipt not an object")?;
    change(object);
    write_json_new(&path.with_extension("edited"), &value)?;
    fs::rename(path.with_extension("edited"), path).map_err(|e| e.to_string())
}

fn mutate_case(name: &str, receipts: &Path, id: &str) -> Result<bool, String> {
    let receipt = receipts.join(format!("{id}.json"));
    let log = receipts.join(format!("{id}.log"));
    match name {
        "missing-receipt" => fs::remove_file(receipt).map_err(|e| e.to_string())?,
        "missing-log" => fs::remove_file(log).map_err(|e| e.to_string())?,
        "symlink-receipt" => {
            fs::remove_file(&receipt).map_err(|e| e.to_string())?;
            #[cfg(unix)] {
                let target = fs::read_dir(receipts).map_err(|e| e.to_string())?
                    .filter_map(Result::ok).map(|entry| entry.path())
                    .find(|path| path.extension().is_some_and(|ext| ext == "json") && path.file_stem().is_some_and(|stem| stem != id))
                    .ok_or("need second synthetic receipt for symlink control")?;
                std::os::unix::fs::symlink(target, receipt).map_err(|e| e.to_string())?;
            }
            #[cfg(not(unix))] return Err("symlink control is Unix-only".into());
        }
        "symlink-log" => {
            fs::remove_file(&log).map_err(|e| e.to_string())?;
            #[cfg(unix)] {
                let target = fs::read_dir(receipts).map_err(|e| e.to_string())?
                    .filter_map(Result::ok).map(|entry| entry.path())
                    .find(|path| path.extension().is_some_and(|ext| ext == "log") && path.file_stem().is_some_and(|stem| stem != id))
                    .ok_or("need second synthetic log for symlink control")?;
                std::os::unix::fs::symlink(target, log).map_err(|e| e.to_string())?;
            }
            #[cfg(not(unix))] return Err("symlink control is Unix-only".into());
        }
        "running-marker" => write_new(&receipts.join("current-check.json"), b"{}\n")?,
        "modified-log" => fs::write(log, b"modified synthetic log\n").map_err(|e| e.to_string())?,
        "duplicate-result-key" => {
            let bytes = fs::read(&receipt).map_err(|e| e.to_string())?;
            let raw = String::from_utf8(bytes).map_err(|e| e.to_string())?;
            let changed = raw.replacen("\"result\": \"passed\"", "\"result\": \"failed\",\n  \"result\": \"passed\"", 1);
            if changed == raw { return Err("could not create duplicate-key probe".into()); }
            fs::write(receipt, changed).map_err(|e| e.to_string())?;
        }
        "stale-commit" => edit_receipt(receipts, id, |o| { o.insert("candidate_commit".into(), json!("0000000000000000000000000000000000000000")); })?,
        "dirty-flag" => edit_receipt(receipts, id, |o| { o.insert("worktree_dirty".into(), json!(true)); })?,
        "wrong-command" => edit_receipt(receipts, id, |o| { o.insert("command".into(), json!(["synthetic-wrong-command"])); })?,
        "wrong-id" => edit_receipt(receipts, id, |o| { o.insert("check_id".into(), json!("wrong-check")); })?,
        "wrong-profile" => edit_receipt(receipts, id, |o| { o.insert("profile".into(), json!("fast")); })?,
        "wrong-result" => edit_receipt(receipts, id, |o| { o.insert("result".into(), json!("failed")); })?,
        "wrong-log-name" => edit_receipt(receipts, id, |o| { o.insert("output_log".into(), json!("other.log")); })?,
        "wrong-version" => edit_receipt(receipts, id, |o| { o.insert("receipt_version".into(), json!(2)); })?,
        "nonzero-exit" => edit_receipt(receipts, id, |o| { o.insert("exit_code".into(), json!(1)); })?,
        "bool-exit" => edit_receipt(receipts, id, |o| { o.insert("exit_code".into(), json!(false)); })?,
        "float-exit" => edit_receipt(receipts, id, |o| { o.insert("exit_code".into(), json!(0.0)); })?,
        "negative-duration" => edit_receipt(receipts, id, |o| { o.insert("duration_ms".into(), json!(-1)); })?,
        "bool-duration" => edit_receipt(receipts, id, |o| { o.insert("duration_ms".into(), json!(true)); })?,
        "float-duration" => edit_receipt(receipts, id, |o| { o.insert("duration_ms".into(), json!(1.5)); })?,
        "bool-version-probe" => edit_receipt(receipts, id, |o| { o.insert("receipt_version".into(), json!(true)); })?,
        "float-version-probe" => edit_receipt(receipts, id, |o| { o.insert("receipt_version".into(), json!(1.0)); })?,
        "numeric-dirty-probe" => edit_receipt(receipts, id, |o| { o.insert("worktree_dirty".into(), json!(0)); })?,
        _ => return Err(format!("unknown baseline control {name}")),
    }
    Ok(name.ends_with("-probe") || name == "duplicate-result-key")
}

fn run_case(root: &Path, script: &Path, out: &Path, name: &str, commit: &str) -> Result<Value, String> {
    let case = out.join(name);
    fs::create_dir(&case).map_err(|e| format!("{}: {e}", case.display()))?;
    let receipts = case.join("receipts");
    let ids = create_fixture(&root.join("xtask/checks.json"), &receipts, commit)?;
    let probe = mutate_case(name, &receipts, &ids[0])?;
    let argv = vec![
        "python3".to_owned(), "-B".to_owned(), script.display().to_string(),
        "--profile".to_owned(), "extended".to_owned(), "--receipts".to_owned(), receipts.display().to_string(),
    ];
    let started = Instant::now();
    let output = Command::new(&argv[0]).args(&argv[1..]).current_dir(root).output().map_err(|e| e.to_string())?;
    let elapsed_ns = started.elapsed().as_nanos();
    write_new(&case.join("stdout.bin"), &output.stdout)?;
    write_new(&case.join("stderr.bin"), &output.stderr)?;
    let expected_status = if probe { None } else if name == "positive" { Some(0) } else { Some(1) };
    let expectation_match = expected_status.is_none_or(|expected| output.status.code() == Some(expected));
    Ok(json!({
        "name": name, "synthetic": true, "command_executed": false,
        "check_ids": ids, "argv": argv, "exit_code": output.status.code(),
        "success": output.status.success(), "wall_ns": elapsed_ns,
        "expected": match expected_status { Some(0) => "validator acceptance", Some(1) => "validator rejection", _ => "observed separately" },
        "expectation_match": expectation_match,
        "stdout": case.join("stdout.bin"), "stderr": case.join("stderr.bin"),
        "probe": probe,
    }))
}

fn main() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let out = PathBuf::from(args.next().ok_or("usage: receipt_differential DATA_RESULTS_DIR")?);
    if args.next().is_some() { return Err("one output path required".into()); }
    let storage = PathBuf::from(std::env::var_os("FABRIC_STORAGE_ROOT").ok_or("FABRIC_STORAGE_ROOT required")?).canonicalize().map_err(|e| e.to_string())?;
    let scratch = PathBuf::from(std::env::var_os("FABRIC_SCRATCH_ROOT").ok_or("FABRIC_SCRATCH_ROOT required")?).canonicalize().map_err(|e| e.to_string())?;
    if !scratch.starts_with(&storage) || !scratch.is_dir() { return Err("scratch must be an existing directory inside DATA storage".into()); }
    let out_parent = out.parent().ok_or("output parent required")?.canonicalize().map_err(|e| e.to_string())?;
    if !out_parent.starts_with(&storage) || out.file_name().is_none() { return Err("new output must be inside DATA storage".into()); }
    fs::create_dir(&out).map_err(|e| format!("new output directory required: {e}"))?;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().ok_or("workspace root")?.to_path_buf();
    let dirty = git(&root, &["status", "--porcelain"])?;
    if !dirty.is_empty() { return Err(format!("source worktree is dirty: {dirty}")); }
    let commit = git(&root, &["rev-parse", "HEAD"])?;
    let registry = root.join("xtask/checks.json");
    let script = root.join("tools/ci/verify_receipts.py");
    let driver = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/receipt_differential.rs");
    let cases = [
        "positive", "missing-receipt", "missing-log", "symlink-receipt", "symlink-log",
        "running-marker", "stale-commit", "dirty-flag", "wrong-command", "wrong-id",
        "wrong-profile", "wrong-result", "wrong-log-name", "wrong-version", "nonzero-exit",
        "bool-exit", "float-exit", "negative-duration", "bool-duration", "float-duration",
        "modified-log", "duplicate-result-key", "bool-version-probe", "float-version-probe", "numeric-dirty-probe",
    ];
    let mut results = Vec::new();
    for name in cases { results.push(run_case(&root, &script, &out, name, &commit)?); }
    let record = json!({
        "mode": "baseline-reference-only", "synthetic_receipts": true,
        "source_commit": commit, "source_worktree_dirty": false,
        "registry_sha256": xtask::sha256_hex(&fs::read(&registry).map_err(|e| e.to_string())?),
        "python_verifier": script, "driver_sha256": xtask::sha256_hex(&fs::read(&driver).map_err(|e| e.to_string())?),
        "outer_resource_receipt_directory": root.join("target/resource-containment/runs"),
        "outer_launcher_writes_its_stop_receipt_after_this_process_exits": true,
        "expectation_mismatches": results.iter().filter(|r| r["expectation_match"] == false).map(|r| r["name"].clone()).collect::<Vec<_>>(),
        "results": results,
        "interpretation": "Synthetic structural evidence only; never evidence that registered commands ran. Probe outcomes are observations, not assumed failures."
    });
    write_json_new(&out.join("baseline.json"), &record)?;
    println!("synthetic Python baseline recorded at {}", out.join("baseline.json").display());
    Ok(())
}
