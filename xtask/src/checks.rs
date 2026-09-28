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

pub fn run_one(root: &Path, check: &Check) -> Outcome {
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
        return Outcome {
            status: Status::EnvironmentUnavailable,
            exit_code: None,
            output: format!("missing: {}", missing.join(", ")).into_bytes(),
        };
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

/// Run every check of `profile` (or only `only`), print a line per check,
/// write receipts, and return the overall status.
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
    let mut overall = Status::Passed;
    for check in selected {
        let started = Instant::now();
        let outcome = run_one(root, check);
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
        let receipt = receipt(&context, check, &outcome, elapsed);
        let path = receipts.join(format!("{}.json", check.id));
        let text = serde_json::to_string_pretty(&receipt).map_err(|e| e.to_string())?;
        std::fs::write(&path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(overall)
}
