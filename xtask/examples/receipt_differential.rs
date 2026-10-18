//! Synthetic receipt differential and benchmark through the original Python
//! CLI and a separately built Rust candidate; this driver contains no verifier
//! decision rule of its own.

use serde_json::{Map, Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!("git {:?} failed", args));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(bytes)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn write_json_new(path: &Path, value: &Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    write_new(path, &bytes)
}

fn create_fixture(registry: &Path, receipts: &Path, commit: &str) -> Result<Vec<String>, String> {
    create_fixture_sized(registry, receipts, commit, None)
}

fn create_fixture_sized(
    registry: &Path,
    receipts: &Path,
    commit: &str,
    log_size: Option<usize>,
) -> Result<Vec<String>, String> {
    fs::create_dir(receipts).map_err(|e| e.to_string())?;
    let checks = xtask::checks::load(registry)?;
    let selected: Vec<_> = checks
        .into_iter()
        .filter(|c| c.profile == "extended")
        .collect();
    if selected.len() != 10 {
        return Err(format!(
            "expected 10 extended checks, found {}",
            selected.len()
        ));
    }
    let mut ids = Vec::new();
    for check in selected {
        let id = check.id.clone();
        let log_name = format!("{id}.log");
        let log = if let Some(size) = log_size {
            let mut bytes = b"SYNTHETIC; command not executed\n".to_vec();
            if size < bytes.len() {
                return Err("synthetic log size is smaller than its trust header".into());
            }
            bytes.resize(size, b'x');
            bytes
        } else {
            format!("SYNTHETIC RECEIPT FIXTURE; no command executed; check={id}\n").into_bytes()
        };
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

fn edit_receipt(
    receipts: &Path,
    id: &str,
    change: impl FnOnce(&mut Map<String, Value>),
) -> Result<(), String> {
    let path = receipts.join(format!("{id}.json"));
    let mut value: Value = serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let object = value
        .as_object_mut()
        .ok_or("fixture receipt not an object")?;
    change(object);
    write_json_new(&path.with_extension("edited"), &value)?;
    fs::rename(path.with_extension("edited"), path).map_err(|e| e.to_string())
}

fn mutate_case(name: &str, receipts: &Path, id: &str) -> Result<bool, String> {
    let receipt = receipts.join(format!("{id}.json"));
    let log = receipts.join(format!("{id}.log"));
    match name {
        "positive" => {}
        "missing-receipt" => fs::remove_file(receipt).map_err(|e| e.to_string())?,
        "missing-log" => fs::remove_file(log).map_err(|e| e.to_string())?,
        "symlink-receipt" => {
            fs::remove_file(&receipt).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                let target = fs::read_dir(receipts)
                    .map_err(|e| e.to_string())?
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .find(|path| {
                        path.extension().is_some_and(|ext| ext == "json")
                            && path.file_stem().is_some_and(|stem| stem != id)
                    })
                    .ok_or("need second synthetic receipt for symlink control")?;
                std::os::unix::fs::symlink(target, receipt).map_err(|e| e.to_string())?;
            }
            #[cfg(not(unix))]
            return Err("symlink control is Unix-only".into());
        }
        "symlink-log" => {
            fs::remove_file(&log).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                let target = fs::read_dir(receipts)
                    .map_err(|e| e.to_string())?
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .find(|path| {
                        path.extension().is_some_and(|ext| ext == "log")
                            && path.file_stem().is_some_and(|stem| stem != id)
                    })
                    .ok_or("need second synthetic log for symlink control")?;
                std::os::unix::fs::symlink(target, log).map_err(|e| e.to_string())?;
            }
            #[cfg(not(unix))]
            return Err("symlink control is Unix-only".into());
        }
        "running-marker" => write_new(&receipts.join("current-check.json"), b"{}\n")?,
        "modified-log" => fs::write(log, b"modified synthetic log\n").map_err(|e| e.to_string())?,
        "duplicate-result-key" => {
            let bytes = fs::read(&receipt).map_err(|e| e.to_string())?;
            let raw = String::from_utf8(bytes).map_err(|e| e.to_string())?;
            let changed = raw.replacen(
                "\"result\": \"passed\"",
                "\"result\": \"failed\",\n  \"result\": \"passed\"",
                1,
            );
            if changed == raw {
                return Err("could not create duplicate-key probe".into());
            }
            fs::write(receipt, changed).map_err(|e| e.to_string())?;
        }
        "stale-commit" => edit_receipt(receipts, id, |o| {
            o.insert(
                "candidate_commit".into(),
                json!("0000000000000000000000000000000000000000"),
            );
        })?,
        "dirty-flag" => edit_receipt(receipts, id, |o| {
            o.insert("worktree_dirty".into(), json!(true));
        })?,
        "wrong-command" => edit_receipt(receipts, id, |o| {
            o.insert("command".into(), json!(["synthetic-wrong-command"]));
        })?,
        "wrong-id" => edit_receipt(receipts, id, |o| {
            o.insert("check_id".into(), json!("wrong-check"));
        })?,
        "wrong-profile" => edit_receipt(receipts, id, |o| {
            o.insert("profile".into(), json!("fast"));
        })?,
        "wrong-result" => edit_receipt(receipts, id, |o| {
            o.insert("result".into(), json!("failed"));
        })?,
        "wrong-log-name" => edit_receipt(receipts, id, |o| {
            o.insert("output_log".into(), json!("other.log"));
        })?,
        "wrong-version" => edit_receipt(receipts, id, |o| {
            o.insert("receipt_version".into(), json!(2));
        })?,
        "nonzero-exit" => edit_receipt(receipts, id, |o| {
            o.insert("exit_code".into(), json!(1));
        })?,
        "bool-exit" => edit_receipt(receipts, id, |o| {
            o.insert("exit_code".into(), json!(false));
        })?,
        "float-exit" => edit_receipt(receipts, id, |o| {
            o.insert("exit_code".into(), json!(0.0));
        })?,
        "negative-duration" => edit_receipt(receipts, id, |o| {
            o.insert("duration_ms".into(), json!(-1));
        })?,
        "bool-duration" => edit_receipt(receipts, id, |o| {
            o.insert("duration_ms".into(), json!(true));
        })?,
        "float-duration" => edit_receipt(receipts, id, |o| {
            o.insert("duration_ms".into(), json!(1.5));
        })?,
        "bool-version-probe" => edit_receipt(receipts, id, |o| {
            o.insert("receipt_version".into(), json!(true));
        })?,
        "float-version-probe" => edit_receipt(receipts, id, |o| {
            o.insert("receipt_version".into(), json!(1.0));
        })?,
        "numeric-dirty-probe" => edit_receipt(receipts, id, |o| {
            o.insert("worktree_dirty".into(), json!(0));
        })?,
        _ => return Err(format!("unknown baseline control {name}")),
    }
    Ok(name.ends_with("-probe") || name == "duplicate-result-key")
}

fn run_case(
    root: &Path,
    script: &Path,
    out: &Path,
    name: &str,
    commit: &str,
) -> Result<Value, String> {
    let case = out.join(name);
    fs::create_dir(&case).map_err(|e| format!("{}: {e}", case.display()))?;
    let receipts = case.join("receipts");
    let ids = create_fixture(&root.join("xtask/checks.json"), &receipts, commit)?;
    let probe = mutate_case(name, &receipts, &ids[0])?;
    let argv = vec![
        "python3".to_owned(),
        "-B".to_owned(),
        script.display().to_string(),
        "--profile".to_owned(),
        "extended".to_owned(),
        "--receipts".to_owned(),
        receipts.display().to_string(),
    ];
    let started = Instant::now();
    let output = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    let elapsed_ns = started.elapsed().as_nanos();
    write_new(&case.join("stdout.bin"), &output.stdout)?;
    write_new(&case.join("stderr.bin"), &output.stderr)?;
    let expected_status = if probe {
        None
    } else if name == "positive" {
        Some(0)
    } else {
        Some(1)
    };
    let expectation_match =
        expected_status.is_none_or(|expected| output.status.code() == Some(expected));
    Ok(json!({
        "name": name, "synthetic": true, "registered_check_executed": false,
        "check_ids": ids, "argv": argv, "exit_code": output.status.code(),
        "success": output.status.success(), "wall_ns": elapsed_ns,
        "expected": match expected_status { Some(0) => "validator acceptance", Some(1) => "validator rejection", _ => "observed separately" },
        "expectation_match": expectation_match,
        "stdout": case.join("stdout.bin"), "stderr": case.join("stderr.bin"),
        "probe": probe,
    }))
}

fn command_argv(program: &Path, receipts: &Path) -> Vec<String> {
    vec![
        program.display().to_string(),
        "verify-receipts".into(),
        "--profile".into(),
        "extended".into(),
        "--receipts".into(),
        receipts.display().to_string(),
    ]
}

fn python_argv(script: &Path, receipts: &Path) -> Vec<String> {
    let mut argv = vec!["python3".into(), "-B".into(), script.display().to_string()];
    argv.extend([
        "--profile".into(),
        "extended".into(),
        "--receipts".into(),
        receipts.display().to_string(),
    ]);
    argv
}

fn invoke(
    root: &Path,
    argv: &[String],
    evidence_dir: &Path,
    prefix: &str,
    timed: bool,
) -> Result<Value, String> {
    let stdout_path = evidence_dir.join(format!("{prefix}-stdout.bin"));
    let stderr_path = evidence_dir.join(format!("{prefix}-stderr.bin"));
    let time_path = evidence_dir.join(format!("{prefix}-time.txt"));
    let wrapped = if timed {
        let mut wrapped = vec![
            "/usr/bin/time".to_owned(),
            "-f".to_owned(),
            "%U %S %M %x".to_owned(),
            "-o".to_owned(),
            time_path.display().to_string(),
        ];
        wrapped.extend(argv.iter().cloned());
        write_new(&time_path, b"")?;
        Some(wrapped)
    } else {
        None
    };
    let started = Instant::now();
    let (actual_argv, output, launch_error) = if let Some(wrapped) = wrapped {
        let result = Command::new(&wrapped[0])
            .args(&wrapped[1..])
            .current_dir(root)
            .output();
        match result {
            Ok(output) => (wrapped, Some(output), None),
            Err(error) => (wrapped, None, Some(error.to_string())),
        }
    } else {
        let result = Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(root)
            .output();
        match result {
            Ok(output) => (argv.to_vec(), Some(output), None),
            Err(error) => (argv.to_vec(), None, Some(error.to_string())),
        }
    };
    let wall_ns = started.elapsed().as_nanos();
    let stdout = output
        .as_ref()
        .map(|run| run.stdout.as_slice())
        .unwrap_or_default();
    let stderr = output
        .as_ref()
        .map(|run| run.stderr.as_slice())
        .unwrap_or_default();
    write_new(&stdout_path, stdout)?;
    write_new(&stderr_path, stderr)?;
    let (time_raw, time_read_error) = if timed {
        match fs::read_to_string(&time_path) {
            Ok(raw) => (Some(raw), None),
            Err(error) => (None, Some(error.to_string())),
        }
    } else {
        (None, None)
    };
    let metric: Option<Value> = time_raw.as_ref().and_then(|raw| {
        let fields: Vec<_> = raw.split_whitespace().collect();
        (fields.len() == 4).then(|| json!({
            "user_seconds": fields[0], "system_seconds": fields[1],
            "max_rss_kib": fields[2].parse::<u64>().ok(), "timed_exit": fields[3].parse::<i32>().ok()
        }))
    });
    Ok(json!({
        "argv": argv, "timed_wrapper_argv": if timed { Some(actual_argv) } else { None },
        "exit_code": output.as_ref().and_then(|run| run.status.code()),
        "success": output.as_ref().is_some_and(|run| run.status.success()),
        "launch_error": launch_error, "wall_ns": wall_ns,
        "stdout": stdout_path, "stderr": stderr_path,
        "time_raw": if timed { Some(time_path) } else { None }, "time_output": time_raw,
        "time_read_error": time_read_error, "measured_cpu_rss": metric
    }))
}

fn parse_stdout(path: &Path) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))
}

fn normalized_report(path: &Path) -> Result<Value, String> {
    let mut report = parse_stdout(path)?;
    let object = report
        .as_object_mut()
        .ok_or("validator success output is not an object")?;
    object.remove("limits");
    Ok(report)
}

fn run_differential_case(
    root: &Path,
    out: &Path,
    binary: &Path,
    script: &Path,
    registry: &Path,
    commit: &str,
    name: &str,
) -> Result<Value, String> {
    let case = out.join("differential").join(name);
    fs::create_dir_all(&case).map_err(|error| error.to_string())?;
    let receipts = case.join("receipts");
    let ids = create_fixture_sized(registry, &receipts, commit, Some(64))?;
    let probe = mutate_case(name, &receipts, &ids[0])?;
    let python = invoke(
        root,
        &python_argv(script, &receipts),
        &case,
        "python",
        false,
    )?;
    let native = invoke(
        root,
        &command_argv(binary, &receipts),
        &case,
        "native",
        false,
    )?;
    let py_path = PathBuf::from(python["stdout"].as_str().ok_or("stdout path missing")?);
    let native_path = PathBuf::from(native["stdout"].as_str().ok_or("stdout path missing")?);
    let py_code = python["exit_code"].as_i64();
    let native_code = native["exit_code"].as_i64();
    let expected_py = if probe || name == "positive" {
        Some(0)
    } else {
        Some(1)
    };
    let expected_native = if name == "positive" { Some(0) } else { Some(1) };
    let positive_match = if name == "positive" {
        Some(
            match (normalized_report(&py_path), normalized_report(&native_path)) {
                (Ok(py), Ok(native)) => py == native,
                _ => false,
            },
        )
    } else {
        None
    };
    let expected_decisions = py_code == expected_py && native_code == expected_native;
    let parity = if probe {
        py_code == Some(0) && native_code == Some(1)
    } else {
        py_code == native_code
    };
    Ok(json!({
        "name": name, "synthetic": true, "registered_check_executed": false,
        "check_ids": ids, "probe": probe, "python": python, "native": native,
        "expected_python_exit": expected_py, "expected_native_exit": expected_native,
        "expected_decisions": expected_decisions, "common_parity": parity,
        "normalized_positive_reports_match": positive_match,
        "modified_log_defect_canary": name == "modified-log",
        "accepted": expected_decisions && positive_match.unwrap_or(true) && parity
    }))
}

fn validate_candidate_path(binary: &Path, storage: &Path) -> Result<PathBuf, String> {
    if !binary.is_absolute() {
        return Err("candidate ELF path must be absolute".into());
    }
    let metadata = fs::symlink_metadata(binary).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("candidate ELF must be an ordinary non-symlink file".into());
    }
    let canonical = binary.canonicalize().map_err(|error| error.to_string())?;
    if !canonical.starts_with(storage) {
        return Err("candidate ELF must be inside DATA storage".into());
    }
    Ok(canonical)
}

fn bench_trial(root: &Path, out: &Path, prefix: &str, argv: &[String]) -> Result<Value, String> {
    fs::create_dir(out).map_err(|error| error.to_string())?;
    invoke(root, argv, out, prefix, true)
}

fn reports_equal(left: &Value, right: &Value) -> bool {
    let left_path = Path::new(left["stdout"].as_str().unwrap_or(""));
    let right_path = Path::new(right["stdout"].as_str().unwrap_or(""));
    match (normalized_report(left_path), normalized_report(right_path)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn run_benchmarks(
    root: &Path,
    out: &Path,
    binary: &Path,
    script: &Path,
    registry: &Path,
    commit: &str,
) -> Result<Value, String> {
    let corpora = [
        ("64B", 64usize),
        ("64KiB", 64 * 1024),
        ("4MiB", 4 * 1024 * 1024),
    ];
    fs::create_dir(out.join("benchmark")).map_err(|error| error.to_string())?;
    let mut results = Vec::new();
    for (name, size) in corpora {
        let corpus = out.join("benchmark").join(name);
        fs::create_dir(&corpus).map_err(|error| error.to_string())?;
        let receipts = corpus.join("receipts");
        let ids = create_fixture_sized(registry, &receipts, commit, Some(size))?;
        let python = python_argv(script, &receipts);
        let native = command_argv(binary, &receipts);
        let mut corpus_ok = true;
        let mut warmup = Vec::new();
        for (implementation, argv) in [("python", &python), ("native", &native)] {
            let trial = bench_trial(
                root,
                &corpus.join(format!("warmup-{implementation}")),
                implementation,
                argv,
            )?;
            corpus_ok &= trial["exit_code"] == 0;
            warmup.push(json!({"implementation": implementation, "trial": trial}));
        }
        corpus_ok &= reports_equal(&warmup[0]["trial"], &warmup[1]["trial"]);
        let reference = warmup[0]["trial"].clone();
        let mut pairs = Vec::new();
        for repetition in 0..7 {
            let order = if repetition % 2 == 0 {
                ["python", "native"]
            } else {
                ["native", "python"]
            };
            let mut pair = serde_json::Map::new();
            for implementation in order {
                let argv = if implementation == "python" {
                    &python
                } else {
                    &native
                };
                let trial = bench_trial(
                    root,
                    &corpus.join(format!("rep-{repetition}-{implementation}")),
                    implementation,
                    argv,
                )?;
                corpus_ok &= trial["exit_code"] == 0;
                corpus_ok &= reports_equal(&trial, &reference);
                pair.insert(implementation.into(), trial);
            }
            corpus_ok &= reports_equal(&pair["python"], &pair["native"]);
            pairs.push(json!({"repetition": repetition, "order": order, "trials": pair}));
        }
        results.push(json!({
            "corpus": name, "synthetic": true, "registered_check_executed": false,
            "extended_check_count": ids.len(), "log_bytes_per_check": size,
            "total_log_bytes": size * ids.len(), "fixture_generation_outside_timing": true,
            "warmup": warmup, "alternating_pairs": pairs, "decisions_and_reports_match": corpus_ok
        }));
    }
    Ok(Value::Array(results))
}

fn run_candidate(
    root: &Path,
    out: &Path,
    binary_arg: &Path,
    registry: &Path,
    script: &Path,
    commit: &str,
) -> Result<(), String> {
    let storage = PathBuf::from(
        std::env::var_os("FABRIC_STORAGE_ROOT").ok_or("FABRIC_STORAGE_ROOT required")?,
    )
    .canonicalize()
    .map_err(|error| error.to_string())?;
    let binary = validate_candidate_path(binary_arg, &storage)?;
    let binary_len = fs::metadata(&binary)
        .map_err(|error| error.to_string())?
        .len();
    if binary_len > 64 * 1024 * 1024 {
        return Err(format!(
            "candidate ELF exceeds 64 MiB bound: {binary_len} bytes"
        ));
    }
    let binary_bytes = fs::read(&binary).map_err(|error| error.to_string())?;
    if !binary_bytes.starts_with(b"\x7fELF") {
        return Err("candidate file is not an ELF binary".into());
    }
    let binary_hash = xtask::sha256_hex(&binary_bytes);
    let python_hash = xtask::sha256_hex(&fs::read(script).map_err(|error| error.to_string())?);
    let registry_hash = xtask::sha256_hex(&fs::read(registry).map_err(|error| error.to_string())?);
    let out = out.canonicalize().map_err(|error| error.to_string())?;
    let cases = [
        "positive",
        "missing-receipt",
        "missing-log",
        "symlink-receipt",
        "symlink-log",
        "running-marker",
        "stale-commit",
        "dirty-flag",
        "wrong-command",
        "wrong-id",
        "wrong-profile",
        "wrong-result",
        "wrong-log-name",
        "wrong-version",
        "nonzero-exit",
        "bool-exit",
        "float-exit",
        "negative-duration",
        "bool-duration",
        "float-duration",
        "modified-log",
        "duplicate-result-key",
        "bool-version-probe",
        "float-version-probe",
        "numeric-dirty-probe",
    ];
    let mut differential = Vec::new();
    let mut driver_errors = Vec::new();
    for name in cases {
        match run_differential_case(root, &out, &binary, script, registry, commit, name) {
            Ok(result) => differential.push(result),
            Err(error) => {
                driver_errors.push(format!("{name}: {error}"));
                differential.push(json!({"name": name, "accepted": false, "driver_error": error}));
            }
        }
    }
    let differential_ok =
        differential.len() == 25 && differential.iter().all(|case| case["accepted"] == true);
    let benchmark_result = if differential_ok {
        run_benchmarks(root, &out, &binary, script, registry, commit)
    } else {
        Err("benchmark deferred because differential outcomes failed".into())
    };
    let (benchmarks, benchmark_error, benchmark_ok) = match benchmark_result {
        Ok(value) => {
            let ok = value.as_array().is_some_and(|corpora| {
                corpora.len() == 3
                    && corpora
                        .iter()
                        .all(|c| c["decisions_and_reports_match"] == true)
            });
            (value, Value::Null, ok)
        }
        Err(error) => (Value::Array(Vec::new()), json!(error), false),
    };
    let source_unchanged = git(root, &["rev-parse", "HEAD"]).ok().as_deref() == Some(commit)
        && git(root, &["status", "--porcelain"]).is_ok_and(|status| status.is_empty());
    let binary_unchanged = fs::read(&binary)
        .ok()
        .is_some_and(|bytes| xtask::sha256_hex(&bytes) == binary_hash);
    let verifier_inputs_unchanged = fs::read(script)
        .ok()
        .is_some_and(|bytes| xtask::sha256_hex(&bytes) == python_hash)
        && fs::read(registry)
            .ok()
            .is_some_and(|bytes| xtask::sha256_hex(&bytes) == registry_hash);
    let common_parity_cases: Vec<_> = differential
        .iter()
        .filter(|case| case["probe"] != true)
        .map(|case| json!({"name": case["name"], "parity": case["common_parity"]}))
        .collect();
    let strict_probe_cases: Vec<_> = differential.iter().filter(|case| case["probe"] == true)
        .map(|case| json!({"name": case["name"], "python_exit": case["python"]["exit_code"],
            "native_exit": case["native"]["exit_code"], "expected_python_exit": case["expected_python_exit"],
            "expected_native_exit": case["expected_native_exit"]})).collect();
    let decision_mismatches: Vec<_> = differential
        .iter()
        .filter(|case| case["expected_decisions"] != true)
        .map(|case| case["name"].clone())
        .collect();
    let positive_passed = differential
        .iter()
        .find(|case| case["name"] == "positive")
        .is_some_and(|case| {
            case["python"]["exit_code"] == 0
                && case["native"]["exit_code"] == 0
                && case["normalized_positive_reports_match"] == true
        });
    let modified_log_rejected = differential
        .iter()
        .find(|case| case["name"] == "modified-log")
        .is_some_and(|case| case["python"]["exit_code"] == 1 && case["native"]["exit_code"] == 1);
    let driver_errors_empty = driver_errors.is_empty();
    let record = json!({
        "mode": "python-rust-differential-and-benchmark", "synthetic_receipts": true,
        "source_commit_before": commit, "source_commit_after": git(root, &["rev-parse", "HEAD"]).unwrap_or_default(),
        "source_worktree_clean_and_unchanged": source_unchanged,
        "candidate_elf": binary, "candidate_elf_sha256_before": binary_hash,
        "candidate_elf_unchanged": binary_unchanged,
        "python_verifier": script, "python_verifier_sha256": python_hash,
        "registry_sha256": registry_hash, "verifier_inputs_unchanged": verifier_inputs_unchanged,
        "common_case_count": 21, "strict_representation_probe_count": 4,
        "common_parity_cases": common_parity_cases, "strict_probe_cases": strict_probe_cases,
        "unexpected_decision_mismatches": decision_mismatches,
        "differential_cases": differential, "differential_passed": differential_ok,
        "defect_canary": {"positive_case_passed": positive_passed, "modified_log_case_rejected": modified_log_rejected,
            "skip_hash_mutant_expected_mismatch_case": "modified-log"},
        "benchmark": benchmarks, "benchmark_error": benchmark_error,
        "benchmark_passed": benchmark_ok,
        "timing_boundary": "Includes process start and verifier work; fixture creation is outside the measured interval; compilation is excluded.",
        "gnu_time": "/usr/bin/time -f '%U %S %M %x'", "driver_errors": driver_errors,
        "overall_status": if differential_ok && benchmark_ok && source_unchanged && binary_unchanged && verifier_inputs_unchanged && driver_errors_empty { "completed" } else { "failed" },
        "interpretation": "Synthetic structural differential and bounded host measurements only; no command execution or performance generalization."
    });
    write_json_new(&out.join("candidate-report.json"), &record)?;
    println!(
        "candidate differential report: {}",
        out.join("candidate-report.json").display()
    );
    if !differential_ok
        || !benchmark_ok
        || !source_unchanged
        || !binary_unchanged
        || !verifier_inputs_unchanged
        || !driver_errors_empty
    {
        return Err(
            "candidate differential, benchmark, source or ELF invariant failed; evidence retained"
                .into(),
        );
    }
    Ok(())
}

fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let (candidate, out) = if args.first().is_some_and(|arg| arg == "--candidate") {
        if args.len() != 3 {
            return Err("usage: receipt_differential --candidate ABS_ELF DATA_RESULTS_DIR".into());
        }
        (Some(PathBuf::from(&args[1])), PathBuf::from(&args[2]))
    } else {
        if args.len() != 1 {
            return Err("usage: receipt_differential DATA_RESULTS_DIR".into());
        }
        (None, PathBuf::from(&args[0]))
    };
    let storage = PathBuf::from(
        std::env::var_os("FABRIC_STORAGE_ROOT").ok_or("FABRIC_STORAGE_ROOT required")?,
    )
    .canonicalize()
    .map_err(|e| e.to_string())?;
    let scratch = PathBuf::from(
        std::env::var_os("FABRIC_SCRATCH_ROOT").ok_or("FABRIC_SCRATCH_ROOT required")?,
    )
    .canonicalize()
    .map_err(|e| e.to_string())?;
    if !scratch.starts_with(&storage) || !scratch.is_dir() {
        return Err("scratch must be an existing directory inside DATA storage".into());
    }
    let out_parent = out
        .parent()
        .ok_or("output parent required")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !out_parent.starts_with(&storage) || out.file_name().is_none() {
        return Err("new output must be inside DATA storage".into());
    }
    fs::create_dir(&out).map_err(|e| format!("new output directory required: {e}"))?;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("workspace root")?
        .to_path_buf();
    let dirty = git(&root, &["status", "--porcelain"])?;
    if !dirty.is_empty() {
        return Err(format!("source worktree is dirty: {dirty}"));
    }
    let commit = git(&root, &["rev-parse", "HEAD"])?;
    let registry = root.join("xtask/checks.json");
    let script = root.join("tools/ci/verify_receipts.py");
    let driver = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/receipt_differential.rs");
    if let Some(binary) = candidate {
        run_candidate(&root, &out, &binary, &registry, &script, &commit)?;
        return Ok(());
    }
    run_baseline(&root, &out, &registry, &script, &driver, &commit)
}

fn run_baseline(
    root: &Path,
    out: &Path,
    registry: &Path,
    script: &Path,
    driver: &Path,
    commit: &str,
) -> Result<(), String> {
    let cases = [
        "positive",
        "missing-receipt",
        "missing-log",
        "symlink-receipt",
        "symlink-log",
        "running-marker",
        "stale-commit",
        "dirty-flag",
        "wrong-command",
        "wrong-id",
        "wrong-profile",
        "wrong-result",
        "wrong-log-name",
        "wrong-version",
        "nonzero-exit",
        "bool-exit",
        "float-exit",
        "negative-duration",
        "bool-duration",
        "float-duration",
        "modified-log",
        "duplicate-result-key",
        "bool-version-probe",
        "float-version-probe",
        "numeric-dirty-probe",
    ];
    let mut results = Vec::new();
    for name in cases {
        results.push(run_case(&root, &script, &out, name, &commit)?);
    }
    let source_unchanged = git(root, &["rev-parse", "HEAD"])? == commit
        && git(root, &["status", "--porcelain"])?.is_empty();
    let mismatches: Vec<_> = results
        .iter()
        .filter(|r| r["expectation_match"] == false)
        .map(|r| r["name"].clone())
        .collect();
    let controls_match = mismatches.is_empty();
    let record = json!({
        "mode": "baseline-reference-only", "synthetic_receipts": true,
        "source_commit": commit, "source_worktree_dirty": false,
        "registry_sha256": xtask::sha256_hex(&fs::read(&registry).map_err(|e| e.to_string())?),
        "python_verifier": script, "python_verifier_sha256": xtask::sha256_hex(&fs::read(&script).map_err(|e| e.to_string())?),
        "driver_sha256": xtask::sha256_hex(&fs::read(&driver).map_err(|e| e.to_string())?),
        "source_unchanged_at_end": source_unchanged,
        "outer_resource_receipt_directory": root.join("target/resource-containment/runs"),
        "outer_launcher_writes_its_stop_receipt_after_this_process_exits": true,
        "expectation_mismatches": mismatches,
        "results": results,
        "interpretation": "Synthetic structural evidence only; never evidence that registered commands ran. Probe outcomes are observations, not assumed failures."
    });
    write_json_new(&out.join("baseline.json"), &record)?;
    println!(
        "synthetic Python baseline recorded at {}",
        out.join("baseline.json").display()
    );
    if !controls_match || !source_unchanged {
        return Err(
            "baseline controls or final source identity mismatch; evidence retained".into(),
        );
    }
    Ok(())
}
