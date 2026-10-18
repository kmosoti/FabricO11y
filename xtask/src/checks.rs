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
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

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
) -> Outcome {
    if let Some(outcome) = unavailable(root, check) {
        return outcome;
    }
    let execute = || -> Result<(std::process::ExitStatus, Vec<u8>), std::io::Error> {
        let status = Command::new(&check.command[0])
            .args(&check.command[1..])
            .current_dir(root)
            .stdout(out)
            .stderr(err)
            .status()?;
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
        }
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
    json!({
        "receipt_version": 1,
        "check_id": check.id,
        "property_ids": field("contracts"),
        "candidate_commit": context.commit,
        "worktree_dirty": context.dirty,
        "specification_revision": revision_of(&context.root, &files("specification")),
        "oracle_revision": revision_of(&context.root, &files("oracles")),
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
        let stdout_name = format!("{}.stdout.partial.log", check.id);
        let stderr_name = format!("{}.stderr.partial.log", check.id);
        let stdout_path = receipts.join(&stdout_name);
        let stderr_path = receipts.join(&stderr_name);
        // Truncate old streams before publishing a new running identity.
        let stdout = std::fs::File::create(&stdout_path).map_err(|e| e.to_string())?;
        let stderr = std::fs::File::create(&stderr_path).map_err(|e| e.to_string())?;
        // A stopped command leaves this marker, never a fabricated success.
        // Completed logs/receipts remain available when a later check fails.
        write_record(
            &marker,
            &json!({
                "state": "running", "check_id": check.id,
                "candidate_commit": context.commit, "command": check.command,
                "stdout_log": stdout_name, "stderr_log": stderr_name,
            }),
        )?;
        println!("{:<24} running  {}", check.id, check.command.join(" "));
        std::io::stdout().flush().map_err(|e| e.to_string())?;
        let started = Instant::now();
        let outcome = run_one_recorded(root, check, stdout, stderr, &stdout_path, &stderr_path);
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
        overall = match (overall, outcome.status) {
            (Status::Failed, _) | (_, Status::Failed) => Status::Failed,
            (Status::EnvironmentUnavailable, _) | (_, Status::EnvironmentUnavailable) => {
                Status::EnvironmentUnavailable
            }
            _ => Status::Passed,
        };
        let log_name = format!("{}.log", check.id);
        let log = receipts.join(&log_name);
        std::fs::write(&log, &outcome.output).map_err(|e| format!("{}: {e}", log.display()))?;
        let mut receipt = receipt(&context, check, &outcome, elapsed);
        receipt["output_log"] = json!(log_name);
        write_record(&receipts.join(format!("{}.json", check.id)), &receipt)?;
        std::fs::remove_file(&marker).map_err(|e| format!("{}: {e}", marker.display()))?;
    }
    Ok(overall)
}

#[cfg(test)]
mod shard_workflow_contract {
    use serde_json::Value;

    const EXPECTED_IDS: [&str; 10] = [
        "semantic-mutants",
        "delivery-faults",
        "tla-delivery",
        "tla-transport",
        "trace-model",
        "cargo-mutants",
        "kani-core",
        "fuzz-smoke",
        "dependency-policy",
        "coverage-report",
    ];

    fn assignment_arrays(source: &str) -> Result<Vec<Vec<String>>, String> {
        let mut remaining = source;
        let mut arrays = Vec::new();
        while let Some((_, after)) = remaining.split_once("checks=(") {
            let (body, tail) = after.split_once(')').ok_or("unterminated checks array")?;
            arrays.push(body.split_whitespace().map(str::to_owned).collect());
            remaining = tail;
        }
        Ok(arrays)
    }

    fn has_line(section: &str, expected: &str) -> bool {
        section.lines().any(|line| line.trim() == expected)
    }

    fn contract(source: &str) -> Result<(), String> {
        let registry: Value = serde_json::from_str(include_str!("../checks.json"))
            .map_err(|error| error.to_string())?;
        let registered: Vec<String> = registry["checks"]
            .as_array()
            .ok_or("registry checks missing")?
            .iter()
            .filter(|entry| entry["profile"] == "extended")
            .map(|entry| entry["id"].as_str().unwrap_or("").to_owned())
            .collect();
        let expected: Vec<String> = EXPECTED_IDS.iter().map(|id| (*id).to_owned()).collect();
        if registered != expected {
            return Err("extended registry IDs changed; review shard ownership".into());
        }
        let arrays = assignment_arrays(source)?;
        if arrays.len() != 2
            || arrays[0] != vec!["semantic-mutants".to_owned()]
            || arrays[1]
                != EXPECTED_IDS[1..]
                    .iter()
                    .map(|id| (*id).to_owned())
                    .collect::<Vec<_>>()
        {
            return Err("shard assignments must own the exact ten extended IDs once".into());
        }
        let matrix = source
            .split_once("    strategy:\n")
            .and_then(|(_, rest)| rest.split_once("    steps:\n").map(|(matrix, _)| matrix))
            .ok_or("shard matrix section missing")?;
        if matrix
            .lines()
            .filter(|line| line.trim() == "fail-fast: false")
            .count()
            != 1
            || matrix
                .lines()
                .filter(|line| line.trim() == "max-parallel: 2")
                .count()
                != 1
        {
            return Err("shard matrix must be exactly bounded to two and non-fail-fast".into());
        }
        let producer = source
            .split_once("      - name: Run assigned original checks and retain later outcomes\n")
            .map(|(_, section)| section)
            .ok_or("serial producer step missing")?;
        if !has_line(
            producer,
            "python3 -B tools/resource_group.py -- cargo xtask checks --only \"$check\" --receipts \"$SHARD_RECEIPTS\"",
        ) {
            return Err("producer must use the original contained check command".into());
        }
        let receipt_artifact = source
            .split_once("      - name: Keep shard receipts\n")
            .and_then(|(_, rest)| {
                rest.split_once("      - name: Keep runner and resource containment facts\n")
                    .map(|(section, _)| section)
            })
            .ok_or("receipt artifact step missing")?;
        if !has_line(receipt_artifact, "name: pilot-receipts-${{ matrix.shard }}")
            || !has_line(
                receipt_artifact,
                "path: ${{ env.FABRIC_STORAGE_ROOT }}/results/verification-producer/${{ github.run_id }}-${{ github.run_attempt }}-${{ matrix.shard }}/",
            )
        {
            return Err("each shard must upload its explicit DATA receipt directory".into());
        }
        let aggregate = source
            .split_once("\n  extended:\n")
            .map(|(_, section)| section)
            .ok_or("required extended aggregate job missing")?;
        let aggregate_header = aggregate
            .split_once("    steps:\n")
            .map(|(header, _)| header)
            .ok_or("aggregate steps missing")?;
        if !has_line(aggregate_header, "needs: shards")
            || !has_line(
                aggregate_header,
                "if: ${{ always() && (github.event_name != 'pull_request' || github.event.pull_request.draft == false) }}",
            )
        {
            return Err(
                "aggregate must always join producers except for draft pull requests".into(),
            );
        }
        let aggregate_body = aggregate.split_once("    steps:\n").unwrap().1;
        let download_step = aggregate_body
            .split_once("      - name: Download both receipt shards\n")
            .map(|(_, section)| section)
            .and_then(|section| {
                section
                    .split_once("      - name: Require both named receipt directories\n")
                    .map(|(step, _)| step)
            })
            .ok_or("receipt download step missing")?;
        if !has_line(download_step, "pattern: pilot-receipts-*")
            || !has_line(
                download_step,
                "path: ${{ env.FABRIC_STORAGE_ROOT }}/results/verification-pilot/shards",
            )
            || !has_line(download_step, "merge-multiple: false")
        {
            return Err("aggregate must download both separate shard artifacts to DATA".into());
        }
        let merge_step = aggregate_body
            .split_once("      - name: Merge exact shard evidence without overwrites\n")
            .map(|(_, section)| section)
            .and_then(|section| {
                section
                    .split_once(
                        "      - name: Verify complete extended receipts at this revision\n",
                    )
                    .map(|(step, _)| step)
            })
            .ok_or("exact receipt merge step missing")?;
        if !has_line(merge_step, "SHARD_RESULT: ${{ needs.shards.result }}")
            || !has_line(
                merge_step,
                "run: python3 -B tools/resource_group.py -- cargo run --locked -p xtask --example merge_verification_receipts -- --shard-result \"$SHARD_RESULT\" \"$FABRIC_STORAGE_ROOT/results/verification-pilot/shards\" \"$FABRIC_STORAGE_ROOT/results/verification-pilot/combined\"",
            )
        {
            return Err(
                "aggregate must merge exact current shard outputs with producer result".into(),
            );
        }
        let validator_step = aggregate_body
            .split_once("      - name: Verify complete extended receipts at this revision\n")
            .map(|(_, section)| section)
            .and_then(|section| {
                section
                    .split_once("      - name: Keep aggregate evidence and runner facts\n")
                    .map(|(step, _)| step)
            })
            .ok_or("complete-profile validator step missing")?;
        if !has_line(
            validator_step,
            "run: python3 -B tools/resource_group.py -- python3 -B tools/ci/verify_receipts.py --profile extended --receipts \"$FABRIC_STORAGE_ROOT/results/verification-pilot/combined\"",
        ) {
            return Err("aggregate must invoke the unchanged complete-profile validator".into());
        }
        let status_step = aggregate_body
            .split_once("      - name: Require both matrix shards to have succeeded\n")
            .map(|(_, section)| section)
            .ok_or("producer status step missing")?;
        if !has_line(status_step, "if: always()")
            || !has_line(status_step, "SHARD_RESULT: ${{ needs.shards.result }}")
            || !has_line(status_step, "if [ \"$SHARD_RESULT\" != success ]; then")
        {
            return Err("aggregate must reject any unsuccessful producer result".into());
        }
        Ok(())
    }

    #[test]
    fn hosted_shards_preserve_exact_extended_ownership_and_required_aggregate() {
        contract(include_str!("../../.github/workflows/verification.yml")).unwrap();
    }

    #[test]
    fn representative_workflow_mutations_are_rejected() {
        let original = include_str!("../../.github/workflows/verification.yml");
        let mutations = [
            (
                "command-wrapper",
                "python3 -B tools/resource_group.py -- cargo xtask checks --only \"$check\" --receipts \"$SHARD_RECEIPTS\"",
                "cargo xtask checks --only \"$check\" --receipts \"$SHARD_RECEIPTS\"",
            ),
            ("missing-check", " coverage-report)", ")"),
            (
                "extra-check",
                "checks=(semantic-mutants)",
                "checks=(semantic-mutants extra-check)",
            ),
            ("worker-bound", "max-parallel: 2", "max-parallel: 3"),
            (
                "worker-bound-over-two-digits",
                "max-parallel: 2",
                "max-parallel: 20",
            ),
            ("fail-fast", "fail-fast: false", "fail-fast: true"),
            (
                "missing-validator",
                "tools/ci/verify_receipts.py --profile extended",
                "tools/ci/verify_receipts.py --profile fast",
            ),
            (
                "producer-status",
                "if [ \"$SHARD_RESULT\" != success ]; then",
                "if [ \"$SHARD_RESULT\" = success ]; then",
            ),
            (
                "aggregate-needs",
                "    needs: shards\n",
                "    needs: other\n",
            ),
            (
                "aggregate-always",
                "    if: ${{ always() && (github.event_name != 'pull_request' || github.event.pull_request.draft == false) }}",
                "    if: ${{ success() && (github.event_name != 'pull_request' || github.event.pull_request.draft == false) }}",
            ),
            (
                "aggregate-validator-removed",
                "      - name: Verify complete extended receipts at this revision\n",
                "      - name: Verify changed receipts at this revision\n",
            ),
        ];
        for (name, from, to) in mutations {
            assert!(
                original.contains(from),
                "mutation precondition missing for {name}"
            );
            let changed = original.replacen(from, to, 1);
            assert!(
                contract(&changed).is_err(),
                "workflow mutation accepted: {name}"
            );
        }
    }

    #[test]
    fn retired_pilot_workflow_is_manual_only() {
        let pilot = include_str!("../../.github/workflows/verification-concurrency-experiment.yml");
        let triggers = pilot.split_once("\nconcurrency:\n").unwrap().0;
        assert!(triggers.contains("\non:\n  workflow_dispatch:\n"));
        assert!(!triggers.contains("pull_request:"));
        assert!(!triggers.contains("schedule:"));
        assert!(!triggers.contains("merge_group:"));
    }
}
