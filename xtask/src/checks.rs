//! The required-check registry and its runner.
//!
//! `xtask/checks.json` lists every required repository check: command, scope,
//! required tools, expected result, related contracts and profile. The
//! runner executes one profile, reports each check as passed, failed or
//! environment-unavailable, and writes one verification receipt per check.
//! A receipt shows what the runner observed; it is unsigned, so it proves the
//! record's structure, not that the command ran.

use crate::sha256_hex;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::OpenOptionsExt;
use std::collections::HashMap;
use std::sync::{Mutex, atomic::{AtomicUsize, Ordering}};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const LIGHT_WORKER_COUNT: usize = 2;
const LIGHT_COHORT: &[(&str, u64, &[&str])] = &[
    ("delivery-oracle", 310, &["python3", "-B", "tools/qualification/test_delivery_oracle.py"]),
    ("query-oracle", 322, &["python3", "-B", "tools/qualification/test_query_oracle.py"]),
    ("workload-rate-oracle", 887, &["python3", "-B", "tools/qualification/test_workload.py"]),
    ("soak-decision-rule", 87, &["python3", "-B", "tools/qualification/test_soak_tier.py"]),
    ("telemetry-contract", 142, &["python3", "-B", "tools/telemetry/test_contract.py"]),
    ("telemetry-cli", 2241, &["python3", "-B", "tools/telemetry/test_cli.py"]),
    ("ui-palette", 42, &["python3", "-B", "tools/ui/contrast_check.py"]),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Passed,
    Failed,
    EnvironmentUnavailable,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Passed => "passed",
            Status::Failed => "failed",
            Status::EnvironmentUnavailable => "environment-unavailable",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Check {
    pub id: String,
    pub profile: String,
    pub command: Vec<String>,
    pub requires: Vec<String>,
    pub requires_paths: Vec<String>,
    pub raw: Value,
}

pub fn load(path: &Path) -> Result<Vec<Check>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let json: Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let strings = |v: &Value, key: &str| -> Vec<String> {
        v.get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut checks = Vec::new();
    for entry in json
        .get("checks")
        .and_then(Value::as_array)
        .ok_or("check registry: missing `checks` array")?
    {
        let text = |key: &str| {
            entry
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| format!("check registry: entry without `{key}`"))
        };
        for key in ["scope", "expected", "unchecked"] {
            text(key)?;
        }
        let check = Check {
            id: text("id")?,
            profile: text("profile")?,
            command: strings(entry, "command"),
            requires: strings(entry, "requires"),
            requires_paths: strings(entry, "requires_paths"),
            raw: entry.clone(),
        };
        if check.command.is_empty() {
            return Err(format!("check registry: `{}` has no command", check.id));
        }
        if checks.iter().any(|c: &Check| c.id == check.id) {
            return Err(format!("check registry: duplicate id `{}`", check.id));
        }
        checks.push(check);
    }
    Ok(checks)
}

pub fn on_path(tool: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(tool).is_file()))
}

fn capture(root: &Path, program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|| "unavailable".into())
}

/// The last commit that touched any of `files`, or `none`.
fn revision_of(root: &Path, files: &[String]) -> String {
    if files.is_empty() {
        return "none".into();
    }
    let mut args = vec!["log", "-1", "--format=%H", "--"];
    args.extend(files.iter().map(String::as_str));
    capture(root, "git", &args)
}

pub struct Outcome {
    pub status: Status,
    pub exit_code: Option<i32>,
    pub output: Vec<u8>,
}

fn unavailable(root: &Path, check: &Check) -> Option<Outcome> {
    let missing: Vec<_> = check
        .requires
        .iter()
        .filter(|tool| !on_path(tool))
        .chain(
            check
                .requires_paths
                .iter()
                .filter(|path| !root.join(path).exists()),
        )
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Some(Outcome {
            status: Status::EnvironmentUnavailable,
            exit_code: None,
            output: format!("missing: {}", missing.join(", ")).into_bytes(),
        });
    }
    None
}

pub fn run_one(root: &Path, check: &Check) -> Outcome {
    if let Some(outcome) = unavailable(root, check) {
        return outcome;
    }
    match Command::new(&check.command[0])
        .args(&check.command[1..])
        .current_dir(root)
        .output()
    {
        Ok(output) => {
            let mut bytes = output.stdout;
            bytes.extend_from_slice(&output.stderr);
            Outcome {
                status: if output.status.success() {
                    Status::Passed
                } else {
                    Status::Failed
                },
                exit_code: output.status.code(),
                output: bytes,
            }
        }
        Err(error) => Outcome {
            status: Status::EnvironmentUnavailable,
            exit_code: None,
            output: format!("cannot start {}: {error}", check.command[0]).into_bytes(),
        },
    }
}

/// Persist each stream before waiting for a check. If the launcher interrupts
/// the registry, these files survive without manufacturing a completion receipt.
fn run_one_recorded(
    root: &Path,
    check: &Check,
    out: std::fs::File,
    err: std::fs::File,
    stdout: &Path,
    stderr: &Path,
    temp: Option<&Path>,
) -> Outcome {
    if let Some(outcome) = unavailable(root, check) {
        return outcome;
    }
    let execute = || -> Result<(std::process::ExitStatus, Vec<u8>), std::io::Error> {
        let mut command = Command::new(&check.command[0]);
        command
            .args(&check.command[1..])
            .current_dir(root)
            .stdout(out)
            .stderr(err);
        if let Some(temp) = temp {
            command.env("TMPDIR", temp).env("TMP", temp).env("TEMP", temp);
        }
        let status = command.status()?;
        // Preserve the existing stdout-then-stderr order and receipt digest.
        let mut bytes = std::fs::read(stdout)?;
        bytes.extend_from_slice(&std::fs::read(stderr)?);
        Ok((status, bytes))
    };
    match execute() {
        Ok((status, output)) => Outcome {
            status: if status.success() {
                Status::Passed
            } else {
                Status::Failed
            },
            exit_code: status.code(),
            output,
        },
        Err(error) => Outcome {
            status: Status::EnvironmentUnavailable,
            exit_code: None,
            output: format!("cannot execute or retain {}: {error}", check.command[0]).into_bytes(),
        },
    }
}

pub struct Context {
    pub root: PathBuf,
    pub commit: String,
    pub dirty: bool,
    pub toolchain: Value,
    bounded_metadata: bool,
    bounded_revisions: HashMap<Vec<String>, String>,
}

impl Context {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            commit: capture(root, "git", &["rev-parse", "HEAD"]),
            dirty: !capture(root, "git", &["status", "--porcelain"]).is_empty(),
            toolchain: json!({
                "rustc": capture(root, "rustc", &["-V"]),
                "cargo": capture(root, "cargo", &["-V"]),
                "python3": capture(root, "python3", &["--version"]),
                "bun": capture(root, "bun", &["--version"]),
            }),
            bounded_metadata: false,
            bounded_revisions: HashMap::new(),
        }
    }

    fn new_bounded(root: &Path, checks: &[Check]) -> Result<Self, String> {
        let mut bounded_revisions = HashMap::new();
        for check in checks {
            for key in ["specification", "oracles"] {
                let files: Vec<String> = check.raw.get(key).and_then(Value::as_array)
                    .map(|array| array.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                    .unwrap_or_default();
                if !bounded_revisions.contains_key(&files) {
                    let value = bounded_revision_of(root, &files).unwrap_or_else(|_| "unavailable".into());
                    bounded_revisions.insert(files, value);
                }
            }
        }
        Ok(Self {
            root: root.to_path_buf(),
            commit: bounded_capture(root, "git", &["rev-parse", "HEAD"] )?,
            dirty: !bounded_capture(root, "git", &["status", "--porcelain"] )?.is_empty(),
            toolchain: json!({
                "rustc": bounded_capture_or_unavailable(root, "rustc", &["-V"]),
                "cargo": bounded_capture_or_unavailable(root, "cargo", &["-V"]),
                "python3": bounded_capture_or_unavailable(root, "python3", &["--version"]),
                "bun": bounded_capture_or_unavailable(root, "bun", &["--version"]),
            }),
            bounded_metadata: true,
            bounded_revisions,
        })
    }
}

pub fn receipt(context: &Context, check: &Check, outcome: &Outcome, elapsed_ms: u128) -> Value {
    let field = |key: &str| check.raw.get(key).cloned().unwrap_or(Value::Null);
    let files = |key: &str| -> Vec<String> {
        check
            .raw
            .get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let revision = |key: &str| {
        let paths = files(key);
        if context.bounded_metadata { context.bounded_revisions.get(&paths).cloned().unwrap_or_else(|| "unavailable".into()) }
        else { revision_of(&context.root, &paths) }
    };
    json!({
        "receipt_version": 1,
        "check_id": check.id,
        "property_ids": field("contracts"),
        "candidate_commit": context.commit,
        "worktree_dirty": context.dirty,
        "specification_revision": revision("specification"),
        "oracle_revision": revision("oracles"),
        "toolchain": context.toolchain,
        "command": check.command,
        "profile": check.profile,
        "scope": field("scope"),
        "fixtures": field("fixtures"),
        "bounds": field("bounds"),
        "negative_controls": field("negative_controls"),
        "expected": field("expected"),
        "result": outcome.status.label(),
        "exit_code": outcome.exit_code,
        "duration_ms": elapsed_ms,
        "output_sha256": sha256_hex(&outcome.output),
        "unchecked_behavior": field("unchecked"),
        "trust": "written by cargo xtask checks; unsigned; proves structure, not execution",
    })
}

fn bounded_capture_or_unavailable(root: &Path, program: &str, arguments: &[&str]) -> String {
    bounded_capture(root, program, arguments).unwrap_or_else(|_| "unavailable".into())
}

fn bounded_capture(root: &Path, program: &str, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("timeout")
        .args(["--signal=KILL", "10s", program])
        .args(arguments)
        .current_dir(root)
        .output()
        .map_err(|error| format!("bounded metadata command unavailable ({program}): {error}"))?;
    if !output.status.success() {
        return Err(format!("bounded metadata command failed ({program}): {:?}: {}", output.status.code(), String::from_utf8_lossy(&output.stderr)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn bounded_revision_of(root: &Path, files: &[String]) -> Result<String, String> {
    if files.is_empty() { return Ok("none".into()); }
    let mut arguments = vec!["log", "-1", "--format=%H", "--"];
    arguments.extend(files.iter().map(String::as_str));
    bounded_capture(root, "git", &arguments)
}

fn write_record(path: &Path, value: &Value) -> Result<(), String> {
    let pending = path.with_extension("pending");
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(&pending, text + "\n").map_err(|e| format!("{}: {e}", pending.display()))?;
    std::fs::rename(&pending, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Run every check of `profile` (or only `only`), print progress, retain complete
/// output and receipts, and return the overall status. Output preserves the
/// existing stdout-then-stderr order; it is not an interleaved live transcript.
pub fn run(
    root: &Path,
    registry: &Path,
    profile: &str,
    only: Option<&str>,
    receipts: &Path,
) -> Result<Status, String> {
    let checks = load(registry)?;
    let selected: Vec<_> = checks
        .iter()
        .filter(|c| only.map_or(c.profile == profile, |id| c.id == id))
        .collect();
    if selected.is_empty() {
        return Err(format!(
            "no checks selected (profile {profile}, only {only:?})"
        ));
    }
    std::fs::create_dir_all(receipts).map_err(|e| format!("{}: {e}", receipts.display()))?;
    let context = Context::new(root);
    let marker = receipts.join("current-check.json");
    let mut overall = Status::Passed;
    for check in selected {
        let (_, status) = run_recorded_check(root, &context, check, receipts, &marker, None)?;
        overall = combine_status(overall, status);
    }
    Ok(overall)
}

fn combine_status(left: Status, right: Status) -> Status {
    match (left, right) {
        (Status::Failed, _) | (_, Status::Failed) => Status::Failed,
        (Status::EnvironmentUnavailable, _) | (_, Status::EnvironmentUnavailable) => {
            Status::EnvironmentUnavailable
        }
        _ => Status::Passed,
    }
}

fn run_recorded_check(
    root: &Path,
    context: &Context,
    check: &Check,
    receipts: &Path,
    marker: &Path,
    temp: Option<&Path>,
) -> Result<(u128, Status), String> {
    let stdout_name = format!("{}.stdout.partial.log", check.id);
    let stderr_name = format!("{}.stderr.partial.log", check.id);
    let stdout_path = receipts.join(&stdout_name);
    let stderr_path = receipts.join(&stderr_name);
    let stdout = std::fs::File::create(&stdout_path).map_err(|e| e.to_string())?;
    let stderr = std::fs::File::create(&stderr_path).map_err(|e| e.to_string())?;
    write_record(
        marker,
        &json!({
            "state": "running", "check_id": check.id,
            "candidate_commit": context.commit, "command": check.command,
            "stdout_log": stdout_name, "stderr_log": stderr_name,
        }),
    )?;
    println!("{:<24} running  {}", check.id, check.command.join(" "));
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let started = Instant::now();
    let outcome = run_one_recorded(root, check, stdout, stderr, &stdout_path, &stderr_path, temp);
    let elapsed = started.elapsed().as_millis();
    println!(
        "{:<24} {:<24} {:>8} ms  {}",
        check.id,
        outcome.status.label(),
        elapsed,
        check.command.join(" ")
    );
    if outcome.status != Status::Passed {
        let text = String::from_utf8_lossy(&outcome.output);
        let tail: Vec<_> = text.lines().rev().take(15).collect();
        for line in tail.into_iter().rev() {
            println!("    | {line}");
        }
    }
    let log_name = format!("{}.log", check.id);
    let log = receipts.join(&log_name);
    std::fs::write(&log, &outcome.output).map_err(|e| format!("{}: {e}", log.display()))?;
    let mut value = receipt(context, check, &outcome, elapsed);
    value["output_log"] = json!(log_name);
    write_record(&receipts.join(format!("{}.json", check.id)), &value)?;
    std::fs::remove_file(marker).map_err(|e| format!("{}: {e}", marker.display()))?;
    Ok((elapsed, outcome.status))
}

fn bounded_git(root: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("timeout")
        .args(["--signal=KILL", "10s", "git"])
        .args(arguments)
        .current_dir(root)
        .output()
        .map_err(|error| format!("bounded Git query unavailable: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Git query failed or timed out: {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_owned())
        .map_err(|error| error.to_string())
}

fn clean_identity(root: &Path) -> Result<String, String> {
    let commit = bounded_git(root, &["rev-parse", "HEAD"])?;
    if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Git returned an invalid commit identity".into());
    }
    if !bounded_git(root, &["status", "--porcelain"])?.is_empty() {
        return Err("source worktree is dirty".into());
    }
    Ok(commit)
}

fn require_same_clean_identity(expected: &str, actual: &str, dirty: bool) -> Result<(), String> {
    if dirty {
        return Err("source worktree is dirty".into());
    }
    if actual != expected {
        return Err("source commit changed during light-worker verification".into());
    }
    Ok(())
}

fn ensure_same_clean_identity(root: &Path, expected: &str) -> Result<(), String> {
    let actual = bounded_git(root, &["rev-parse", "HEAD"])?;
    let dirty = !bounded_git(root, &["status", "--porcelain"])?.is_empty();
    require_same_clean_identity(expected, &actual, dirty)
}

fn validate_light_output(storage: &Path, receipts: &Path) -> Result<PathBuf, String> {
    if !receipts.is_absolute() {
        return Err("light-worker receipts path must be absolute".into());
    }
    let name = receipts
        .file_name()
        .filter(|name| name != "." && name != "..")
        .ok_or("named receipt directory required")?;
    let parent = receipts
        .parent()
        .ok_or("receipt parent required")?
        .canonicalize()
        .map_err(|error| format!("receipt parent: {error}"))?;
    if !parent.starts_with(storage) || !parent.is_dir() {
        return Err("receipt parent must be an existing directory inside DATA".into());
    }
    let output = parent.join(name);
    match std::fs::symlink_metadata(&output) {
        Ok(_) => Err("light-worker receipts directory must be new".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(output),
        Err(error) => Err(format!("receipt path: {error}")),
    }
}

fn safe_receipt_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes[0].is_ascii_lowercase()
        && bytes.iter().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

fn ordered_light_cohort(selected: &[Check]) -> Result<Vec<Check>, String> {
    let mut cohort = Vec::with_capacity(LIGHT_COHORT.len());
    for &(id, _, command) in LIGHT_COHORT {
        let check = selected
            .iter()
            .find(|check| check.id == id)
            .ok_or_else(|| format!("light cohort check missing: {id}"))?;
        if check.profile != "fast" || check.command.iter().map(String::as_str).collect::<Vec<_>>() != command {
            return Err(format!("registered command drift for light cohort check `{id}`"));
        }
        cohort.push(check.clone());
    }
    let expected: std::collections::HashSet<_> = LIGHT_COHORT.iter().map(|(id, _, _)| *id).collect();
    let selected_cohort: std::collections::HashSet<_> = selected
        .iter()
        .filter(|check| expected.contains(check.id.as_str()))
        .map(|check| check.id.as_str())
        .collect();
    if selected_cohort.len() != expected.len() {
        return Err("light cohort membership changed".into());
    }
    sort_light_weighted(&mut cohort);
    Ok(cohort)
}

fn sort_light_weighted(checks: &mut [Check]) {
    checks.sort_by(|left, right| {
        let left_weight = LIGHT_COHORT.iter().find(|(id, _, _)| *id == left.id.as_str()).map_or(0, |item| item.1);
        let right_weight = LIGHT_COHORT.iter().find(|(id, _, _)| *id == right.id.as_str()).map_or(0, |item| item.1);
        right_weight.cmp(&left_weight).then_with(|| left.id.cmp(&right.id))
    });
}

enum LightStep<'a> {
    Serial(&'a Check),
    Cohort,
}

fn light_schedule<'a>(selected: &'a [Check], cohort: &[Check]) -> Vec<LightStep<'a>> {
    let mut plan = Vec::with_capacity(selected.len());
    let mut cohort_inserted = false;
    for check in selected {
        if cohort.iter().any(|item| item.id == check.id) {
            if !cohort_inserted {
                plan.push(LightStep::Cohort);
                cohort_inserted = true;
            }
        } else {
            plan.push(LightStep::Serial(check));
        }
    }
    plan
}

fn run_worker_check(
    root: &Path,
    context: &Context,
    check: &Check,
    stage: &Path,
) -> Result<(u128, Status), String> {
    let temporary = stage.join("tmp");
    std::fs::create_dir(&temporary).map_err(|error| error.to_string())?;
    run_recorded_check(
        root,
        context,
        check,
        stage,
        &stage.join("current-check.json"),
        Some(&temporary),
    )
}

#[derive(Debug)]
struct WorkerResult {
    id: String,
    status: Option<Status>,
    error: Option<String>,
}

fn peak_update(peak: &AtomicUsize, active: usize) {
    let mut previous = peak.load(Ordering::Relaxed);
    while active > previous {
        match peak.compare_exchange_weak(previous, active, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(actual) => previous = actual,
        }
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|message| (*message).to_owned()))
        .unwrap_or_else(|| "worker panicked".into())
}

fn dispatch_light_cohort(
    root: &Path,
    context: &Context,
    cohort: &[Check],
    stage_root: &Path,
) -> (Vec<WorkerResult>, usize, Vec<String>) {
    let queue = Mutex::new(std::collections::VecDeque::from(cohort.to_vec()));
    let results = Mutex::new(Vec::with_capacity(cohort.len()));
    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let mut dispatch_errors = Vec::new();

    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..LIGHT_WORKER_COUNT {
            let spawned = std::thread::Builder::new().name("xtask-light-check".into()).spawn_scoped(scope, || {
                loop {
                    let check = queue.lock().unwrap_or_else(|error| error.into_inner()).pop_front();
                    let Some(check) = check else { break };
                    let now = active.fetch_add(1, Ordering::Relaxed) + 1;
                    peak_update(&peak, now);
                    let stage = stage_root.join(&check.id);
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        run_worker_check(root, context, &check, &stage)
                    }));
                    active.fetch_sub(1, Ordering::Relaxed);
                    let record = match result {
                        Ok(Ok((_elapsed_ms, status))) => WorkerResult {
                            id: check.id,
                            status: Some(status),
                            error: None,
                        },
                        Ok(Err(error)) => WorkerResult {
                            id: check.id,
                            status: None,
                            error: Some(error),
                        },
                        Err(payload) => WorkerResult {
                            id: check.id,
                            status: None,
                            error: Some(panic_message(payload)),
                        },
                    };
                    results.lock().unwrap_or_else(|error| error.into_inner()).push(record);
                }
            });
            match spawned {
                Ok(handle) => handles.push(handle),
                Err(error) => dispatch_errors.push(format!("cannot start cohort worker: {error}")),
            }
        }
        for handle in handles {
            if let Err(payload) = handle.join() {
                dispatch_errors.push(panic_message(payload));
            }
        }
        // If both thread starts failed, or a worker panicked outside a job,
        // drain remaining entries serially but keep the run nonpass.
        loop {
            let check = queue.lock().unwrap_or_else(|error| error.into_inner()).pop_front();
            let Some(check) = check else { break };
            let stage = stage_root.join(&check.id);
            let record = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_worker_check(root, context, &check, &stage)
            })) {
                Ok(Ok((_elapsed_ms, status))) => WorkerResult {
                    id: check.id,
                    status: Some(status),
                    error: None,
                },
                Ok(Err(error)) => WorkerResult {
                    id: check.id,
                    status: None,
                    error: Some(error),
                },
                Err(payload) => WorkerResult {
                    id: check.id,
                    status: None,
                    error: Some(panic_message(payload)),
                },
            };
            results.lock().unwrap_or_else(|error| error.into_inner()).push(record);
        }
    });
    let mut completed = results.into_inner().unwrap_or_else(|error| error.into_inner());
    completed.sort_by_key(|result| cohort.iter().position(|check| check.id == result.id).unwrap_or(usize::MAX));
    (completed, peak.load(Ordering::Relaxed), dispatch_errors)
}

fn create_batch_marker(receipts: &Path, context: &Context, checks: &[Check]) -> Result<PathBuf, String> {
    let marker = receipts.join("current-check.json");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
        .map_err(|error| format!("cannot create batch marker: {error}"))?;
    let checks: Vec<_> = checks
        .iter()
        .map(|check| json!({"check_id": check.id, "command": check.command}))
        .collect();
    let bytes = serde_json::to_vec_pretty(&json!({
        "state": "running", "batch": "light-workers-2",
        "candidate_commit": context.commit, "checks": checks
    }))
    .map_err(|error| error.to_string())?;
    let mut file = file;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    Ok(marker)
}

fn ordinary_digest(source: &Path) -> Result<String, String> {
    let before = std::fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if !before.is_file() || before.file_type().is_symlink() || before.len() > 64 * 1024 * 1024 {
        return Err(format!("source artifact is not an ordinary bounded file: {}", source.display()));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let mut input = options.open(source).map_err(|error| error.to_string())?;
    let opened = input.metadata().map_err(|error| error.to_string())?;
    let identity = |metadata: &std::fs::Metadata| {
        (metadata.dev(), metadata.ino(), metadata.len(), metadata.mtime(), metadata.mtime_nsec(), metadata.ctime(), metadata.ctime_nsec())
    };
    if !opened.is_file() || identity(&opened) != identity(&before) {
        return Err(format!("source artifact changed before digest: {}", source.display()));
    }
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    input.read_to_end(&mut bytes).map_err(|error| error.to_string())?;
    let after_path = std::fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if bytes.len() as u64 != opened.len()
        || identity(&input.metadata().map_err(|error| error.to_string())?) != identity(&opened)
        || identity(&after_path) != identity(&opened)
    {
        return Err(format!("source artifact changed during digest: {}", source.display()));
    }
    Ok(sha256_hex(&bytes))
}

fn copy_ordinary_new(source: &Path, destination: &Path, expected_digest: &str) -> Result<(), String> {
    let before = std::fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if !before.is_file() || before.file_type().is_symlink() {
        return Err(format!("source artifact is not an ordinary file: {}", source.display()));
    }
    if before.len() > 64 * 1024 * 1024 {
        return Err(format!("source artifact exceeds verifier limit: {}", source.display()));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let mut input = options.open(source).map_err(|error| error.to_string())?;
    let opened = input.metadata().map_err(|error| error.to_string())?;
    let identity = |metadata: &std::fs::Metadata| {
        (metadata.dev(), metadata.ino(), metadata.len(), metadata.mtime(), metadata.mtime_nsec(), metadata.ctime(), metadata.ctime_nsec())
    };
    if !opened.is_file() || identity(&opened) != identity(&before) {
        return Err(format!("source artifact changed before copy: {}", source.display()));
    }
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| format!("exclusive publication failed for {}: {error}", destination.display()))?;
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    input.read_to_end(&mut bytes).map_err(|error| error.to_string())?;
    if bytes.len() as u64 != opened.len() {
        return Err(format!("source artifact length changed during copy: {}", source.display()));
    }
    let digest = sha256_hex(&bytes);
    if digest != expected_digest {
        return Err(format!("source artifact changed after verification: {}", source.display()));
    }
    output.write_all(&bytes).map_err(|error| error.to_string())?;
    output.sync_all().map_err(|error| error.to_string())?;
    let after_path = std::fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if identity(&input.metadata().map_err(|error| error.to_string())?) != identity(&opened)
        || identity(&after_path) != identity(&opened)
    {
        return Err(format!("source artifact changed during copy: {}", source.display()));
    }
    let destination_bytes = std::fs::read(destination).map_err(|error| error.to_string())?;
    if sha256_hex(&destination_bytes) != digest {
        return Err(format!("published artifact digest mismatch: {}", destination.display()));
    }
    Ok(())
}

/// Optional bounded dispatcher. The ordinary `run` path remains serial.
pub fn run_with_light_workers(
    root: &Path,
    registry: &Path,
    receipts: &Path,
) -> Result<Status, String> {
    crate::preflight::require_limits()?;
    let storage = PathBuf::from(std::env::var_os("FABRIC_STORAGE_ROOT").ok_or("explicit DATA storage required")?)
        .canonicalize().map_err(|error| error.to_string())?;
    let scratch = PathBuf::from(std::env::var_os("FABRIC_SCRATCH_ROOT").ok_or("explicit DATA scratch required")?)
        .canonicalize().map_err(|error| error.to_string())?;
    if !scratch.starts_with(&storage) || !scratch.is_dir() {
        return Err("launcher scratch must be an existing directory inside DATA".into());
    }
    let receipts = validate_light_output(&storage, receipts)?;
    let commit = clean_identity(root)?;
    let checks = load(registry)?;
    let selected: Vec<_> = checks.into_iter().filter(|check| check.profile == "fast").collect();
    if selected.is_empty() {
        return Err("fast profile has no checks".into());
    }
    if selected.iter().any(|check| !safe_receipt_id(&check.id)) {
        return Err("fast registry contains a noncanonical receipt ID".into());
    }
    let cohort = ordered_light_cohort(&selected)?;
    let context = Context::new_bounded(root, &selected)?;
    if context.commit != commit || context.dirty {
        return Err("source changed while initializing check context".into());
    }
    std::fs::create_dir(&receipts).map_err(|error| format!("new receipts directory required: {error}"))?;
    let run_id = format!("verification-light-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).map_err(|error| error.to_string())?.as_nanos());
    let stage_root = scratch.join(&run_id);
    let marker = create_batch_marker(&receipts, &context, &selected)?;
    if ensure_same_clean_identity(root, &commit).is_err() {
        return Err("source changed while creating batch marker".into());
    }
    std::fs::create_dir(&stage_root).map_err(|error| format!("cannot create unique stage root; marker retained: {error}"))?;
    let mut stage_errors = Vec::new();
    for check in &selected {
        if let Err(error) = std::fs::create_dir(stage_root.join(&check.id)) {
            stage_errors.push(format!("cannot create stage for {}: {error}", check.id));
        }
    }
    let mut overall = Status::Passed;
    let mut execution_errors = stage_errors;
    let mut cohort_started = false;
    for step in light_schedule(&selected, &cohort) {
        match step {
        LightStep::Cohort => {
            cohort_started = true;
            let (results, peak, dispatch_errors) = dispatch_light_cohort(root, &context, &cohort, &stage_root);
            println!("light-worker cohort maximum active workers: {peak}");
            if peak > LIGHT_WORKER_COUNT || results.len() != cohort.len() || !dispatch_errors.is_empty() {
                execution_errors.push(format!("light-worker dispatch incomplete: {} results, errors {:?}", results.len(), dispatch_errors));
            }
            let mut result_ids = std::collections::HashSet::new();
            for (item, result) in cohort.iter().zip(&results) {
                if item.id != result.id || !result_ids.insert(result.id.as_str()) {
                    execution_errors.push("light-worker result inventory mismatch".into());
                }
                if let Some(error) = &result.error {
                    execution_errors.push(format!("light-worker {} incomplete: {error}", result.id));
                    overall = combine_status(overall, Status::Failed);
                }
                if let Some(status) = result.status {
                    overall = combine_status(overall, status);
                }
            }
        }
        LightStep::Serial(check) => {
            let stage = stage_root.join(&check.id);
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_worker_check(root, &context, check, &stage)
            })) {
                Ok(Ok((_, status))) => overall = combine_status(overall, status),
                Ok(Err(error)) => {
                    execution_errors.push(format!("check {} could not be recorded: {error}", check.id));
                    overall = combine_status(overall, Status::Failed);
                }
                Err(payload) => {
                    execution_errors.push(format!("check {} panicked: {}", check.id, panic_message(payload)));
                    overall = combine_status(overall, Status::Failed);
                }
            }
        }
        }
    }
    if !cohort_started {
        execution_errors.push("light cohort was not scheduled".into());
    }
    if !execution_errors.is_empty() {
        eprintln!("light-worker failures retained under {}: {execution_errors:?}", stage_root.display());
        overall = combine_status(overall, Status::Failed);
    }
    if let Err(error) = ensure_same_clean_identity(root, &commit) {
        return Err(format!("source changed during light-worker verification; marker and stage retained at {}: {error}", stage_root.display()));
    }
    if overall != Status::Passed {
        eprintln!("light-worker run did not pass; per-check receipts and logs retained at {}", stage_root.display());
        return Ok(overall);
    }
    let assembled = stage_root.join("assembled");
    std::fs::create_dir(&assembled).map_err(|error| format!("cannot create assembled receipts; marker retained: {error}"))?;
    for check in &selected {
        let stage = stage_root.join(&check.id);
        for name in [format!("{}.stdout.partial.log", check.id), format!("{}.stderr.partial.log", check.id), format!("{}.log", check.id), format!("{}.json", check.id)] {
            let source = stage.join(&name);
            let digest = ordinary_digest(&source)
                .map_err(|error| format!("assembly digest failed; marker retained: {error}"))?;
            copy_ordinary_new(&source, &assembled.join(&name), &digest)
                .map_err(|error| format!("assembly failed; marker retained: {error}"))?;
        }
    }
    ensure_same_clean_identity(root, &commit)
        .map_err(|error| format!("source changed before native fast receipt validation: {error}"))?;
    let verified = crate::receipts::verify(root, "fast", &commit, &assembled)
        .map_err(|error| format!("native verification failed; marker and stage retained at {}: {error}", stage_root.display()))?;
    if verified.get("status").and_then(Value::as_str) != Some("passed")
        || verified.get("check_count").and_then(Value::as_u64) != Some(selected.len() as u64)
    {
        return Err("native receipt verifier returned an incomplete fast inventory".into());
    }
    ensure_same_clean_identity(root, &commit)
        .map_err(|error| format!("source changed during native receipt validation: {error}"))?;
    for check in &selected {
        for name in [format!("{}.stdout.partial.log", check.id), format!("{}.stderr.partial.log", check.id), format!("{}.log", check.id), format!("{}.json", check.id)] {
            let source = assembled.join(&name);
            let digest = ordinary_digest(&source)
                .map_err(|error| format!("publication digest failed; marker retained: {error}"))?;
            copy_ordinary_new(&source, &receipts.join(&name), &digest)
                .map_err(|error| format!("canonical publication failed; marker retained: {error}"))?;
        }
    }
    std::fs::File::open(&receipts).and_then(|directory| directory.sync_all())
        .map_err(|error| format!("receipt directory sync failed; marker retained: {error}"))?;
    ensure_same_clean_identity(root, &commit)
        .map_err(|error| format!("source changed before canonical receipt publication: {error}"))?;
    std::fs::remove_dir_all(&stage_root).map_err(|error| format!("stage cleanup failed; marker retained: {error}"))?;
    std::fs::remove_file(&marker).map_err(|error| format!("batch marker removal failed: {error}"))?;
    std::fs::File::open(&receipts).and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())?;
    Ok(Status::Passed)
}

#[cfg(test)]
mod light_worker_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let root = PathBuf::from(std::env::var_os("FABRIC_SCRATCH_ROOT").expect("DATA scratch"));
            let path = root.join(format!("light-{label}-{}-{}", std::process::id(), NEXT.fetch_add(1, AtomicOrdering::Relaxed)));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if std::thread::panicking() {
                eprintln!("failed light-worker fixture retained: {}", self.0.display());
            } else {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    fn synthetic_check(id: &str, command: &[&str], requires: &[&str]) -> Check {
        Check {
            id: id.into(),
            profile: "fast".into(),
            command: command.iter().map(|part| (*part).into()).collect(),
            requires: requires.iter().map(|part| (*part).into()).collect(),
            requires_paths: Vec::new(),
            raw: json!({}),
        }
    }

    #[test]
    fn frozen_cohort_commands_and_weight_order_are_exact() {
        let registry = Path::new(env!("CARGO_MANIFEST_DIR")).join("checks.json");
        let selected: Vec<_> = load(&registry).unwrap().into_iter().filter(|check| check.profile == "fast").collect();
        let cohort = ordered_light_cohort(&selected).unwrap();
        assert_eq!(
            cohort.iter().map(|check| check.id.as_str()).collect::<Vec<_>>(),
            vec!["telemetry-cli", "workload-rate-oracle", "query-oracle", "delivery-oracle", "telemetry-contract", "soak-decision-rule", "ui-palette"]
        );
        let mut tied = vec![
            synthetic_check("zeta", &["sh"], &[]),
            synthetic_check("alpha", &["sh"], &[]),
        ];
        sort_light_weighted(&mut tied);
        assert_eq!(tied.iter().map(|check| check.id.as_str()).collect::<Vec<_>>(), vec!["alpha", "zeta"]);
        let mut drifted = selected.clone();
        drifted.iter_mut().find(|check| check.id == "ui-palette").unwrap().command.push("--wrong".into());
        assert!(ordered_light_cohort(&drifted).unwrap_err().contains("command drift"));

        let plan = light_schedule(&selected, &cohort);
        let mut expanded = Vec::new();
        let mut insertion = None;
        for (position, step) in plan.iter().enumerate() {
            match step {
                LightStep::Serial(check) => expanded.push(check.id.as_str()),
                LightStep::Cohort => {
                    insertion = Some(position);
                    expanded.extend(cohort.iter().map(|check| check.id.as_str()));
                }
            }
        }
        assert_eq!(expanded.len(), selected.len());
        assert_eq!(expanded.iter().collect::<std::collections::HashSet<_>>().len(), selected.len());
        let first_light = selected.iter().position(|check| check.id == "workload-rate-oracle").unwrap();
        assert_eq!(insertion, Some(first_light));
        assert!(expanded.contains(&"ui-palette"));
    }

    #[test]
    fn fresh_receipt_root_must_be_absolute_and_data_owned() {
        let scratch = Scratch::new("output-policy");
        let storage = PathBuf::from(std::env::var_os("FABRIC_STORAGE_ROOT").expect("DATA storage")).canonicalize().unwrap();
        assert!(validate_light_output(&storage, Path::new("relative/receipts")).is_err());
        assert!(validate_light_output(&storage, &Path::new("/tmp/light-outside-data")).is_err());
        let fresh = scratch.0.join("receipts");
        assert_eq!(validate_light_output(&storage, &fresh).unwrap(), fresh);
        std::fs::create_dir(&fresh).unwrap();
        assert!(validate_light_output(&storage, &fresh).unwrap_err().contains("must be new"));
        assert!(safe_receipt_id("light-check-2"));
        assert!(!safe_receipt_id("../outside"));
        assert!(!safe_receipt_id("UpperCase"));
    }

    #[test]
    fn failed_command_and_missing_program_do_not_hide_other_cohort_jobs() {
        let scratch = Scratch::new("worker-controls");
        let root = scratch.0.join("source");
        let stage = scratch.0.join("stage");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&stage).unwrap();
        let checks = vec![
            synthetic_check("actual-exit-7", &["sh", "-c", "sleep 0.05; exit 7"], &[]),
            synthetic_check("missing-program", &["fabric-tool-that-does-not-exist"], &[]),
            synthetic_check("independent-pass", &["sh", "-c", "printf completed"], &[]),
        ];
        for check in &checks {
            std::fs::create_dir(stage.join(&check.id)).unwrap();
        }
        let context = Context {
            root: root.clone(),
            commit: "a".repeat(40),
            dirty: false,
            toolchain: json!({}),
            bounded_metadata: false,
            bounded_revisions: HashMap::new(),
        };
        let (results, peak, dispatch_errors) = dispatch_light_cohort(&root, &context, &checks, &stage);
        assert!(dispatch_errors.is_empty(), "{dispatch_errors:?}");
        assert!(peak > 0 && peak <= LIGHT_WORKER_COUNT);
        assert_eq!(results.len(), checks.len());
        assert_eq!(results.iter().map(|result| result.id.as_str()).collect::<Vec<_>>(), ["actual-exit-7", "missing-program", "independent-pass"]);
        assert_eq!(results[0].status, Some(Status::Failed));
        assert_eq!(results[1].status, Some(Status::EnvironmentUnavailable));
        assert_eq!(results[2].status, Some(Status::Passed));
        assert_eq!(combine_status(results[1].status.unwrap(), results[0].status.unwrap()), Status::Failed);
        for check in &checks {
            let directory = stage.join(&check.id);
            assert!(directory.join(format!("{}.json", check.id)).is_file());
            assert!(!directory.join("current-check.json").exists());
        }
    }

    #[test]
    fn identity_drift_and_corrupt_stage_publication_fail_closed() {
        assert!(require_same_clean_identity("a", "b", false).is_err());
        assert!(require_same_clean_identity("a", "a", true).is_err());
        require_same_clean_identity("a", "a", false).unwrap();

        let scratch = Scratch::new("publication-controls");
        let source = scratch.0.join("source");
        let destination = scratch.0.join("destination");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(source.join("sample.log"), b"ok").unwrap();
        let check = synthetic_check("sample", &["sh"], &[]);
        let context = Context {
            root: source.clone(),
            commit: "a".repeat(40),
            dirty: false,
            toolchain: json!({}),
            bounded_metadata: false,
            bounded_revisions: HashMap::new(),
        };
        let marker = create_batch_marker(&destination, &context, &[check]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(source.join("sample.log"), source.join("linked.log")).unwrap();
        let digest = ordinary_digest(&source.join("sample.log")).unwrap();
        assert!(copy_ordinary_new(&source.join("sample.log"), &destination.join("sample.log"), &digest).is_ok());
        assert!(copy_ordinary_new(&source.join("sample.log"), &destination.join("sample.log"), &digest).is_err(), "publication must be exclusive");
        #[cfg(unix)]
        {
            assert!(copy_ordinary_new(&source.join("linked.log"), &destination.join("linked.log"), &digest).is_err());
            assert!(!destination.join("linked.log").exists());
            let fifo = source.join("fifo.log");
            let status = std::process::Command::new("mkfifo").arg(&fifo).status().unwrap();
            assert!(status.success());
            assert!(ordinary_digest(&fifo).is_err(), "FIFO must be rejected without blocking");
            assert!(marker.is_file(), "failed publication must retain the running marker");
        }
    }
}
