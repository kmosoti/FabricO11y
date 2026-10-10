//! Hand-written semantic mutants, each tied to a product contract.
//!
//! `xtask/mutants.json` names a mutant, the contract it threatens, one exact
//! text replacement in one file, the command of the checker expected to catch
//! it, and a test name that must appear as failed. The runner copies the
//! working tree (tracked and untracked, not ignored files) into
//! `target/semantic-mutants/tree`, never touching the real tree, and keeps
//! each command's output under `target/semantic-mutants/logs`. For every
//! command it first runs the unmutated copy (the positive control must pass),
//! then each mutant from a pristine copy of its file.
//!
//! Classification:
//! - `caught`: the command failed and the named test is among the failures;
//! - `caught-elsewhere`: the command failed without the named test failing;
//! - `survived`: the command passed with the mutant applied;
//! - `stale`: the replacement text no longer occurs exactly once;
//! - `inconclusive`: the unmutated command did not pass, so nothing is learned.

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Mutant {
    pub id: String,
    pub contract: String,
    pub file: String,
    pub find: String,
    pub replace: String,
    pub command: Vec<String>,
    pub expect_failing: String,
}

pub fn load(path: &Path) -> Result<Vec<Mutant>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let json: Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut mutants = Vec::new();
    for entry in json
        .get("mutants")
        .and_then(Value::as_array)
        .ok_or("mutant registry: missing `mutants` array")?
    {
        let text = |key: &str| {
            entry
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| format!("mutant registry: entry without `{key}`"))
        };
        text("incorrect_behavior")?;
        mutants.push(Mutant {
            id: text("id")?,
            contract: text("contract")?,
            file: text("file")?,
            find: text("find")?,
            replace: text("replace")?,
            command: entry
                .get("command")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            expect_failing: text("expect_failing")?,
        });
    }
    Ok(mutants)
}

fn copy_tree(root: &Path, to: &Path) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(to);
    let listing = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(root)
        .output()
        .map_err(|e| format!("git ls-files: {e}"))?;
    for name in listing.stdout.split(|b| *b == 0).filter(|n| !n.is_empty()) {
        let name = String::from_utf8_lossy(name).into_owned();
        let source = root.join(&name);
        if !source.is_file() {
            continue; // deleted in the working tree
        }
        let target = to.join(&name);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::copy(&source, &target).map_err(|e| format!("{name}: {e}"))?;
    }
    Ok(())
}

struct Run {
    passed: bool,
    output: String,
}

fn run(tree: &Path, cargo_target: &Path, command: &[String]) -> Run {
    match Command::new(&command[0])
        .args(&command[1..])
        .current_dir(tree)
        .env("CARGO_TARGET_DIR", cargo_target)
        .output()
    {
        Ok(output) => Run {
            passed: output.status.success(),
            output: String::from_utf8_lossy(&output.stdout).into_owned()
                + &String::from_utf8_lossy(&output.stderr),
        },
        Err(error) => Run {
            passed: false,
            output: format!("cannot start {}: {error}", command[0]),
        },
    }
}

/// Did libtest (or unittest) report `name` as failed?
fn reported_failed(output: &str, name: &str) -> bool {
    output.lines().any(|line| {
        line.contains(name)
            && (line.ends_with("FAILED") || line.starts_with("FAIL:") || line.starts_with("ERROR:"))
    })
}

pub fn run_all(root: &Path, registry: &Path, only: Option<&str>) -> Result<bool, String> {
    let mutants: Vec<_> = load(registry)?
        .into_iter()
        .filter(|m| only.is_none_or(|id| m.id == id))
        .collect();
    if mutants.is_empty() {
        return Err("no mutants selected".into());
    }
    let work = std::env::var_os("FABRIC_SCRATCH_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"))
        .join("semantic-mutants");
    let tree = work.join("tree");
    let cargo_target = work.join("cargo");
    let logs = work.join("logs");
    copy_tree(root, &tree)?;
    // Some tests put scratch directories under the package's own `target/`.
    std::fs::create_dir_all(tree.join("target")).map_err(|e| format!("tree/target: {e}"))?;
    std::fs::create_dir_all(&logs).map_err(|e| format!("{}: {e}", logs.display()))?;
    let mut baselines: BTreeMap<Vec<String>, bool> = BTreeMap::new();
    let mut all_caught = true;
    println!(
        "{:<14} {:<18} {:<34} contract",
        "mutant", "result", "expected failing test"
    );
    for mutant in &mutants {
        let baseline = *baselines.entry(mutant.command.clone()).or_insert_with(|| {
            let outcome = run(&tree, &cargo_target, &mutant.command);
            let _ = std::fs::write(
                logs.join(format!("{}.baseline.txt", mutant.id)),
                &outcome.output,
            );
            outcome.passed
        });
        let path: PathBuf = tree.join(&mutant.file);
        let original =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", mutant.file))?;
        let result = if original.matches(&mutant.find).count() != 1 {
            "stale"
        } else if !baseline {
            "inconclusive"
        } else {
            std::fs::write(&path, original.replacen(&mutant.find, &mutant.replace, 1))
                .map_err(|e| format!("{}: {e}", mutant.file))?;
            let outcome = run(&tree, &cargo_target, &mutant.command);
            let _ = std::fs::write(logs.join(format!("{}.txt", mutant.id)), &outcome.output);
            std::fs::write(&path, &original).map_err(|e| format!("{}: {e}", mutant.file))?;
            if outcome.passed {
                "survived"
            } else if reported_failed(&outcome.output, &mutant.expect_failing) {
                "caught"
            } else {
                "caught-elsewhere"
            }
        };
        all_caught &= result == "caught";
        println!(
            "{:<14} {:<18} {:<34} {}",
            mutant.id, result, mutant.expect_failing, mutant.contract
        );
    }
    Ok(all_caught)
}
