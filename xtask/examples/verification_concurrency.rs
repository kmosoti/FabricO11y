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

const WEIGHTS: [(&str, u64); 7] = [
    ("delivery-oracle", 310),
    ("query-oracle", 322),
    ("workload-rate-oracle", 887),
    ("soak-decision-rule", 87),
    ("telemetry-contract", 142),
    ("telemetry-cli", 2241),
    ("ui-palette", 42),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode { Serial, Fifo, Weighted }

impl Mode {
    fn name(self) -> &'static str {
        match self { Self::Serial => "serial", Self::Fifo => "fifo", Self::Weighted => "weighted" }
    }
    fn workers(self) -> usize { if self == Self::Serial { 1 } else { 2 } }
}

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
    mode: Mode,
    label: &str,
    registry: &Path,
) -> Result<(Vec<ResultRow>, u128, usize), String> {
    let workers = mode.workers();
    if !(1..=2).contains(&workers) || ids.is_empty() {
        return Err("one or two workers and a nonempty inventory required".into());
    }
    let dispatch = if mode == Mode::Weighted {
        if ids.iter().any(|id| id == "injected-exit-7") {
            let real_id = ids.iter().find(|id| id.as_str() != "injected-exit-7")
                .ok_or("weighted control missing real check")?;
            let real_weight = WEIGHTS.iter().find(|entry| entry.0 == real_id.as_str())
                .map(|(_, weight)| *weight).ok_or("no frozen weight for control check")?;
            let control = vec![(real_id.as_str(), real_weight), ("injected-exit-7", 1)];
            weighted_order(ids, &control)?
        } else {
            weighted_order(ids, &WEIGHTS)?
        }
    } else {
        ids.to_vec()
    };
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
            let (dispatch, cursor, active, peak, current, expected, source) =
                (&dispatch, &cursor, &active, &peak, &current, &expected, &source);
            scope.spawn(move || {
                while let Some(id) = dispatch.get(cursor.fetch_add(1, Ordering::Relaxed)) {
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
    let modes = [Mode::Serial, Mode::Fifo, Mode::Weighted];
    let mut warmups = BTreeMap::new();
    for mode in modes {
        let name = mode.name();
        let (rows, elapsed, peak) = execute(out, &ids, mode, &format!("warmup-{name}"), &registry)?;
        if !all_pass(&rows) { return Err(format!("warmup {name} had a failed registered check")); }
        warmups.insert(name, json!({"wall_ms": elapsed, "active_worker_peak": peak,
            "inventory": inventory(&rows), "per_check_ms": durations(&rows)}));
    }
    let reference = warmups["serial"]["inventory"].clone();
    if modes.iter().any(|mode| warmups[mode.name()]["inventory"] != reference) {
        return Err("warmup modes differ in semantic inventory".into());
    }
    let mut triples = Vec::new();
    for repetition in 0..7 {
        let mut order = Vec::new();
        let mut observations = BTreeMap::new();
        for offset in 0..3 {
            let mode = modes[(repetition + offset) % 3];
            let name = mode.name();
            let (rows, elapsed, peak) = execute(out, &ids, mode,
                &format!("rep-{repetition}-{name}"), &registry)?;
            if !all_pass(&rows) || inventory(&rows) != reference {
                return Err(format!("repetition {repetition} mode {name} failed or differed"));
            }
            order.push(name);
            observations.insert(name, json!({"wall_ms": elapsed, "active_worker_peak": peak,
                "inventory": inventory(&rows), "per_check_ms": durations(&rows)}));
        }
        triples.push(json!({"repetition": repetition, "order": order, "modes": observations}));
    }
    let partition_bound = exhaustive_partition_bound(&WEIGHTS)?;
    if partition_bound != 2241 { return Err(format!("frozen model bound changed unexpectedly: {partition_bound}")); }
    fs::write(
        out.join("summary.json"),
        serde_json::to_vec_pretty(&json!({
            "state": "completed", "checks": ids, "inventory": reference,
            "warmups": warmups, "triples": triples,
            "weights_ms": WEIGHTS.iter().map(|(id, weight)| (*id, *weight)).collect::<BTreeMap<_, _>>(),
            "mode_worker_limits": {"serial": Mode::Serial.workers(), "fifo": Mode::Fifo.workers(), "weighted": Mode::Weighted.workers()},
            "weighted_dispatch_order": weighted_order(&ids, &WEIGHTS)?,
            "weighted_model_two_worker_lower_bound_ms": partition_bound,
            "model_partition_ms": {"worker_a": ["telemetry-cli"], "worker_b": [
                "workload-rate-oracle", "query-oracle", "delivery-oracle",
                "telemetry-contract", "soak-decision-rule", "ui-palette"
            ], "makespan": 2241},
            "model_note": "frozen per-check median additive model only; excludes startup, contention and receipt processing",
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
    for mode in [Mode::Serial, Mode::Fifo, Mode::Weighted] {
        let name = mode.name();
        let label = format!("control-{name}");
        let (rows, _, _) = execute(
            out,
            &[IDS[0].into(), "injected-exit-7".into()],
            mode,
            &label,
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
        // Exercise copied-evidence rejection controls once for every measured mode.
        let source = out.join(&label).join(IDS[0]);
        let pristine_receipt = fs::read(source.join("receipts").join(format!("{}.json", IDS[0])))
            .map_err(|e| e.to_string())?;
        let pristine_log = fs::read(source.join("receipts").join(format!("{}.log", IDS[0])))
            .map_err(|e| e.to_string())?;
        for defect in ["missing", "stale-source", "modified-log", "running-marker"] {
            let dest = out.join("evidence-controls").join(name).join(defect);
            let receipts = dest.join("receipts");
            fs::create_dir_all(&receipts).map_err(|e| e.to_string())?;
            let rp = receipts.join(format!("{}.json", IDS[0]));
            fs::write(&rp, &pristine_receipt).map_err(|e| e.to_string())?;
            fs::write(receipts.join(format!("{}.log", IDS[0])), &pristine_log)
                .map_err(|e| e.to_string())?;
            if defect == "missing" { fs::remove_file(&rp).map_err(|e| e.to_string())?; }
            if defect == "stale-source" {
                let mut v: Value = serde_json::from_slice(&pristine_receipt).map_err(|e| e.to_string())?;
                v["candidate_commit"] = json!("stale");
                fs::write(&rp, serde_json::to_vec(&v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            }
            if defect == "modified-log" {
                fs::write(receipts.join(format!("{}.log", IDS[0])), b"changed").map_err(|e| e.to_string())?;
            }
            if defect == "running-marker" {
                fs::write(receipts.join("current-check.json"), b"{}").map_err(|e| e.to_string())?;
            }
            let command: Vec<String> = original["checks"].as_array().ok_or("missing checks")?
                .iter().find(|c| c["id"] == IDS[0]).ok_or("missing check")?["command"].as_array()
                .ok_or("missing command")?.iter()
                .map(|arg| arg.as_str().map(str::to_owned).ok_or("invalid argument"))
                .collect::<Result<_, _>>()?;
            if read_row(&Job { id: IDS[0].into(), receipts }, success_status(), &command, &git_head()?).is_ok() {
                return Err(format!("{name} collector accepted {defect} evidence"));
            }
        }
    }
    Ok(())
}

fn weighted_order(ids: &[String], weights: &[(&str, u64)]) -> Result<Vec<String>, String> {
    if ids.len() > IDS.len() { return Err("weighted selection exceeds frozen seven-job set".into()); }
    let mut table = BTreeMap::new();
    for (id, weight) in weights {
        if *weight == 0 || table.insert(*id, *weight).is_some() {
            return Err("weight table has a zero weight or duplicate ID".into());
        }
    }
    let selected: BTreeMap<_, _> = ids.iter().map(String::as_str).map(|id| (id, ())).collect();
    if selected.len() != ids.len() || selected.len() != table.len()
        || selected.keys().any(|id| !table.contains_key(id))
    {
        return Err("weight table must match the exact selected ID set".into());
    }
    let mut ordered = ids.to_vec();
    ordered.sort_by(|a, b| table[b.as_str()].cmp(&table[a.as_str()]).then_with(|| a.cmp(b)));
    Ok(ordered)
}

fn exhaustive_partition_bound(weights: &[(&str, u64)]) -> Result<u64, String> {
    if weights.len() != IDS.len() { return Err("partition bound is defined for exactly seven frozen jobs".into()); }
    let frozen_ids: Vec<String> = IDS.iter().map(|id| (*id).to_owned()).collect();
    weighted_order(&frozen_ids, weights)?;
    let mut minimum = u64::MAX;
    for assignment in 0..(1usize << weights.len()) {
        let (mut left, mut right) = (0u64, 0u64);
        for (index, (_, weight)) in weights.iter().enumerate() {
            let total = if assignment & (1 << index) == 0 { &mut left } else { &mut right };
            *total = total.checked_add(*weight).ok_or("partition duration overflow")?;
        }
        minimum = minimum.min(left.max(right));
    }
    Ok(minimum)
}

#[cfg(test)]
mod scheduling_tests {
    use super::*;

    #[test]
    fn weights_are_exact_positive_and_permutation_independent() {
        let original: Vec<String> = IDS.iter().map(|id| (*id).to_owned()).collect();
        let forward = weighted_order(&original, &WEIGHTS).unwrap();
        fn permutations(items: &mut [String], at: usize, expected: &[String], count: &mut usize) {
            if at == items.len() {
                assert_eq!(weighted_order(items, &WEIGHTS).unwrap(), expected);
                *count += 1;
                return;
            }
            for index in at..items.len() {
                items.swap(at, index);
                permutations(items, at + 1, expected, count);
                items.swap(at, index);
            }
        }
        let mut permuted = original.clone();
        let mut count = 0;
        permutations(&mut permuted, 0, &forward, &mut count);
        assert_eq!(count, 5040);
        assert_eq!(forward.first().map(String::as_str), Some("telemetry-cli"));
        assert_eq!(forward.iter().collect::<std::collections::BTreeSet<_>>().len(), IDS.len());
        assert!(weighted_order(&original[..6], &WEIGHTS).is_err());
        assert!(weighted_order(&original, &[("extra", 1)]).is_err());
        let mut duplicate = WEIGHTS.to_vec(); duplicate.push(WEIGHTS[0]);
        assert!(weighted_order(&original, &duplicate).is_err());
        let mut zero = WEIGHTS.to_vec(); zero[0].1 = 0;
        assert!(weighted_order(&original, &zero).is_err());
    }

    #[test]
    fn ties_use_canonical_id_order() {
        let ids = vec!["zeta".to_owned(), "alpha".to_owned(), "mu".to_owned()];
        assert_eq!(weighted_order(&ids, &[("zeta", 3), ("alpha", 3), ("mu", 3)]).unwrap(),
            vec!["alpha", "mu", "zeta"]);
    }

    #[test]
    fn exhaustive_two_worker_bound_matches_registered_model() {
        assert_eq!(WEIGHTS.iter().map(|(_, weight)| weight).sum::<u64>(), 4031);
        assert_eq!(exhaustive_partition_bound(&WEIGHTS).unwrap(), 2241);
        assert!(exhaustive_partition_bound(&WEIGHTS[..6]).is_err());
        let mut overflow = WEIGHTS;
        overflow[0].1 = u64::MAX;
        assert!(exhaustive_partition_bound(&overflow).is_err());
    }
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
