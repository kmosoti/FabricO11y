//! Bounded, receipt-preserving scheduler experiment. This is deliberately an
//! example rather than a change to the production checks runner.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

const IDS: [&str; 7] = [
    "delivery-oracle",
    "query-oracle",
    "workload-rate-oracle",
    "soak-decision-rule",
    "telemetry-contract",
    "telemetry-cli",
    "ui-palette",
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has workspace parent")
        .to_path_buf()
}

fn worker(id: &str, registry: &Path, receipts: &Path) -> i32 {
    match xtask::checks::run(&root(), registry, "fast", Some(id), receipts) {
        Ok(xtask::checks::Status::Passed) => 0,
        Ok(xtask::checks::Status::Failed) => 1,
        Ok(xtask::checks::Status::EnvironmentUnavailable) => 3,
        Err(error) => {
            eprintln!("check {id} could not run: {error}");
            2
        }
    }
}

#[derive(Clone)]
struct Job {
    id: String,
    receipts: PathBuf,
}

#[derive(Clone)]
struct ResultRow {
    id: String,
    code: Option<i32>,
    elapsed_ms: u128,
    inventory: Value,
}

fn read_row(
    job: &Job,
    status: std::process::ExitStatus,
    command: &[String],
    source: &Value,
) -> Result<ResultRow, String> {
    let receipt_path = job.receipts.join(format!("{}.json", job.id));
    let receipt: Value = serde_json::from_slice(
        &fs::read(&receipt_path).map_err(|e| format!("{}: {e}", receipt_path.display()))?,
    )
    .map_err(|e| format!("{}: {e}", receipt_path.display()))?;
    if job.receipts.join("current-check.json").exists() {
        return Err(format!("{} left a running marker", job.id));
    }
    let log_name = receipt["output_log"]
        .as_str()
        .ok_or_else(|| format!("{} has no literal output_log", job.id))?;
    if log_name != format!("{}.log", job.id) {
        return Err(format!("{} has unexpected output_log {log_name:?}", job.id));
    }
    let log = fs::read(job.receipts.join(log_name)).map_err(|e| format!("{} log: {e}", job.id))?;
    if receipt["output_sha256"].as_str() != Some(&xtask::sha256_hex(&log)) {
        return Err(format!("{} log digest mismatch", job.id));
    }
    let process_code = status.code();
    let command_code = receipt["exit_code"]
        .as_i64()
        .filter(|n| (0..=255).contains(n))
        .map(|n| n as i32);
    let expected_process_code = match receipt["result"].as_str() {
        Some("passed") if command_code == Some(0) => Some(0),
        Some("failed") if command_code.is_some_and(|code| code != 0) => Some(1),
        Some("environment-unavailable") => Some(3),
        _ => None,
    };
    if receipt["check_id"] != job.id
        || receipt["candidate_commit"] != *source
        || receipt["worktree_dirty"] != false
        || receipt["profile"] != "fast"
        || receipt["command"] != json!(command)
        || process_code != expected_process_code
        || command_code.is_none()
    {
        return Err(format!("{} receipt does not match child outcome", job.id));
    }
    let inventory = json!({
        "id": receipt["check_id"],
        "argv": receipt["command"],
        "source": receipt["candidate_commit"],
        "dirty": receipt["worktree_dirty"],
        "profile": receipt["profile"],
        "result": receipt["result"],
        "exit": receipt["exit_code"],
    });
    Ok(ResultRow {
        id: job.id.clone(),
        code: command_code,
        elapsed_ms: receipt["duration_ms"]
            .as_u64()
            .ok_or("invalid check duration")? as u128,
        inventory,
    })
}

fn execute(
    out: &Path,
    ids: &[String],
    workers: usize,
    label: &str,
    registry: &Path,
) -> Result<(Vec<ResultRow>, u128, usize), String> {
    if !(1..=2).contains(&workers) || ids.is_empty() {
        return Err("one or two workers and a nonempty inventory required".into());
    }
    let cursor = AtomicUsize::new(0);
    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let source = git_head()?;
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    let registry_json: Value =
        serde_json::from_slice(&fs::read(registry).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let entries = registry_json["checks"]
        .as_array()
        .ok_or("registry missing checks")?;
    let mut expected = BTreeMap::<String, Vec<String>>::new();
    for entry in entries {
        let id = entry["id"].as_str().ok_or("check without id")?.to_owned();
        let command = entry["command"]
            .as_array()
            .ok_or("check without command")?
            .iter()
            .map(|v| {
                v.as_str()
                    .ok_or("non-string command item")
                    .map(str::to_owned)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if expected.insert(id, command).is_some() {
            return Err("duplicate registry ID".into());
        }
    }
    let mut unique = ids.to_vec();
    unique.sort();
    unique.dedup();
    if unique.len() != ids.len() || ids.iter().any(|id| !expected.contains_key(id)) {
        return Err("missing or duplicate selected ID".into());
    }
    let mut rows = Vec::new();
    let mut errors = Vec::new();
    let start = Instant::now();
    std::thread::scope(|scope| -> Result<(), String> {
        let (send, receive) = std::sync::mpsc::channel();
        for _ in 0..workers {
            let send = send.clone();
            let (cursor, active, peak, current, expected, source) =
                (&cursor, &active, &peak, &current, &expected, &source);
            scope.spawn(move || {
                while let Some(id) = ids.get(cursor.fetch_add(1, Ordering::Relaxed)) {
                    let result = (|| {
                        let base = out.join(label).join(id);
                        let receipts = base.join("receipts");
                        let scratch = PathBuf::from(
                            std::env::var_os("FABRIC_SCRATCH_ROOT")
                                .ok_or("required launcher scratch root")?,
                        )
                        .join(label)
                        .join(id);
                        let temp = scratch.join("tmp");
                        for path in [&receipts, &scratch, &temp] {
                            fs::create_dir_all(path)
                                .map_err(|e| format!("{}: {e}", path.display()))?;
                        }
                        let job = Job {
                            id: id.clone(),
                            receipts,
                        };
                        let stdout =
                            fs::File::create(base.join("stdout.log")).map_err(|e| e.to_string())?;
                        let stderr =
                            fs::File::create(base.join("stderr.log")).map_err(|e| e.to_string())?;
                        let mut child = Command::new(current)
                            .args([
                                "--worker",
                                id,
                                registry.to_str().ok_or("registry path is not UTF-8")?,
                                job.receipts.to_str().ok_or("receipt path is not UTF-8")?,
                            ])
                            .env("FABRIC_SCRATCH_ROOT", &scratch)
                            .env("TMPDIR", &temp)
                            .env("TMP", &temp)
                            .env("TEMP", &temp)
                            .stdout(Stdio::from(stdout))
                            .stderr(Stdio::from(stderr))
                            .spawn()
                            .map_err(|e| e.to_string())?;
                        let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(now, Ordering::SeqCst);
                        let status = match child.wait() {
                            Ok(status) => Ok(status),
                            Err(error) => {
                                let _ = child.kill();
                                let _ = child.wait();
                                Err(error.to_string())
                            }
                        };
                        active.fetch_sub(1, Ordering::SeqCst);
                        read_row(&job, status?, &expected[id], source)
                    })();
                    if send.send(result).is_err() {
                        break;
                    }
                }
            });
        }
        drop(send);
        for result in receive {
            match result {
                Ok(row) => rows.push(row),
                Err(error) => errors.push(error),
            }
        }
        Ok(())
    })?;
    if rows.len() != ids.len() {
        errors.push("incomplete result inventory".into());
    }
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    Ok((
        rows,
        start.elapsed().as_millis(),
        peak.load(Ordering::SeqCst),
    ))
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|a| a == "--worker") {
        let code = worker(
            args.get(1).and_then(|v| v.to_str()).unwrap_or(""),
            Path::new(args.get(2).and_then(|v| v.to_str()).unwrap_or("")),
            Path::new(args.get(3).and_then(|v| v.to_str()).unwrap_or("")),
        );
        std::process::exit(code);
    }
    let Some(out_arg) = args.first() else {
        eprintln!("usage: verification_concurrency DATA_OUTDIR");
        std::process::exit(2);
    };
    let out = PathBuf::from(out_arg);
    if let Err(error) = parent(&out) {
        eprintln!("experiment failed: {error}");
        std::process::exit(1);
    }
}

fn parent(out: &Path) -> Result<(), String> {
    let storage = fs::canonicalize(
        std::env::var_os("FABRIC_STORAGE_ROOT").ok_or("explicit DATA root required")?,
    )
    .map_err(|e| e.to_string())?;
    let parent = out
        .parent()
        .ok_or("output parent required")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !out.is_absolute() || !parent.starts_with(storage) || out.file_name().is_none() {
        return Err("ordinary output under DATA required".into());
    }
    fs::create_dir(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let registry = root().join("xtask/checks.json");
    let ids: Vec<String> = IDS.iter().map(|s| (*s).to_owned()).collect();
    controls(out, &registry)?;
    // One warmup per mode, discarded only from the timing summary; evidence remains.
    let (one, _, _) = execute(out, &ids, 1, "warmup-one", &registry)?;
    let (two, _, _) = execute(out, &ids, 2, "warmup-two", &registry)?;
    if !all_pass(&one) || !all_pass(&two) || inventory(&one) != inventory(&two) {
        return Err("warmup modes differ or a registered check failed".into());
    }
    let mut pairs = Vec::new();
    for pair in 0..7 {
        let (a, ta, pa, b, tb, pb) = if pair % 2 == 0 {
            let (a, ta, pa) = execute(out, &ids, 1, &format!("pair-{pair}-one"), &registry)?;
            let (b, tb, pb) = execute(out, &ids, 2, &format!("pair-{pair}-two"), &registry)?;
            (a, ta, pa, b, tb, pb)
        } else {
            let (b, tb, pb) = execute(out, &ids, 2, &format!("pair-{pair}-two"), &registry)?;
            let (a, ta, pa) = execute(out, &ids, 1, &format!("pair-{pair}-one"), &registry)?;
            (a, ta, pa, b, tb, pb)
        };
        if !all_pass(&a)
            || !all_pass(&b)
            || inventory(&a) != inventory(&b)
            || inventory(&a) != inventory(&one)
        {
            return Err(format!(
                "pair {pair} has a failed check or semantic inventory mismatch"
            ));
        }
        pairs.push(
            json!({"pair": pair, "one_worker_ms": ta, "two_worker_ms": tb,
            "one_peak": pa, "two_peak": pb,
            "per_check_ms_one": durations(&a), "per_check_ms_two": durations(&b)}),
        );
    }
    fs::write(
        out.join("summary.json"),
        serde_json::to_vec_pretty(&json!({
            "state": "completed", "checks": ids, "inventory": inventory(&one), "pairs": pairs,
            "claim": "observations only; not a performance conclusion"
        }))
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn git_head() -> Result<Value, String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root())
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("git rev-parse failed".into());
    }
    Ok(json!(String::from_utf8_lossy(&output.stdout).trim()))
}

fn controls(out: &Path, registry: &Path) -> Result<(), String> {
    let original: Value = serde_json::from_slice(&fs::read(registry).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let mut fake = original.clone();
    let real = original["checks"]
        .as_array()
        .ok_or("missing checks")?
        .iter()
        .find(|c| c["id"] == IDS[0])
        .ok_or("missing control real check")?
        .clone();
    fake["checks"] = json!([real, {
        "id":"injected-exit-7", "profile":"fast", "command":["sh","-c","exit 7"],
        "scope":"collector control", "expected":"exit 7", "unchecked":"control only"
    }]);
    let fake_path = out.join("control-registry.json");
    fs::write(
        &fake_path,
        serde_json::to_vec(&fake).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    for (label, workers) in [("control-one", 1), ("control-two", 2)] {
        let (rows, _, _) = execute(
            out,
            &[IDS[0].into(), "injected-exit-7".into()],
            workers,
            label,
            &fake_path,
        )?;
        if rows.len() != 2
            || all_pass(&rows)
            || rows
                .iter()
                .find(|r| r.id == IDS[0])
                .is_none_or(|r| r.code != Some(0))
            || rows
                .iter()
                .find(|r| r.id == "injected-exit-7")
                .is_none_or(|r| r.code != Some(7))
        {
            return Err(format!("{label} did not retain both actual child outcomes"));
        }
    }
    // Exercise the collector's four evidence rejection paths using copied artifacts.
    let source = out.join("control-one").join(IDS[0]);
    let pristine_receipt = fs::read(source.join("receipts").join(format!("{}.json", IDS[0])))
        .map_err(|e| e.to_string())?;
    let pristine_log = fs::read(source.join("receipts").join(format!("{}.log", IDS[0])))
        .map_err(|e| e.to_string())?;
    for defect in ["missing", "stale-source", "modified-log", "running-marker"] {
        let dest = out.join("evidence-controls").join(defect);
        let receipts = dest.join("receipts");
        fs::create_dir_all(&receipts).map_err(|e| e.to_string())?;
        let rp = receipts.join(format!("{}.json", IDS[0]));
        fs::write(&rp, &pristine_receipt).map_err(|e| e.to_string())?;
        fs::write(receipts.join(format!("{}.log", IDS[0])), &pristine_log)
            .map_err(|e| e.to_string())?;
        if defect == "missing" {
            fs::remove_file(&rp).map_err(|e| e.to_string())?;
        }
        if defect == "stale-source" {
            let mut v: Value =
                serde_json::from_slice(&pristine_receipt).map_err(|e| e.to_string())?;
            v["candidate_commit"] = json!("stale");
            fs::write(&rp, serde_json::to_vec(&v).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        }
        if defect == "modified-log" {
            fs::write(receipts.join(format!("{}.log", IDS[0])), b"changed")
                .map_err(|e| e.to_string())?;
        }
        if defect == "running-marker" {
            fs::write(receipts.join("current-check.json"), b"{}").map_err(|e| e.to_string())?;
        }
        let command: Vec<String> = original["checks"]
            .as_array()
            .ok_or("missing checks")?
            .iter()
            .find(|c| c["id"] == IDS[0])
            .ok_or("missing check")?["command"]
            .as_array()
            .ok_or("missing command")?
            .iter()
            .map(|arg| arg.as_str().map(str::to_owned).ok_or("invalid argument"))
            .collect::<Result<_, _>>()?;
        let rejected = read_row(
            &Job {
                id: IDS[0].into(),
                receipts,
            },
            success_status(),
            &command,
            &git_head()?,
        )
        .is_err();
        if !rejected {
            return Err(format!("collector accepted {defect} evidence"));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn success_status() -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(0)
}

fn all_pass(rows: &[ResultRow]) -> bool {
    !rows.is_empty()
        && rows
            .iter()
            .all(|r| r.code == Some(0) && r.inventory["result"] == "passed")
}

fn inventory(rows: &[ResultRow]) -> Value {
    Value::Array(rows.iter().map(|r| r.inventory.clone()).collect())
}

fn durations(rows: &[ResultRow]) -> Value {
    json!(
        rows.iter()
            .map(|r| (r.id.as_str(), r.elapsed_ms))
            .collect::<BTreeMap<_, _>>()
    )
}
