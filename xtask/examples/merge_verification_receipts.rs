//! Experimental byte-preserving shard collector. The unchanged independent
//! Python validator, run afterward, decides receipt integrity/completeness.
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const LOG_LIMIT: u64 = 256 * 1024 * 1024;
const JSON_LIMIT: u64 = 4 * 1024 * 1024;

fn ordinary_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !metadata.is_dir() { return Err(format!("ordinary directory required: {}", path.display())); }
    Ok(())
}

fn copy_bounded(source: &Path, destination: &Path) -> Result<(), String> {
    let mut input = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(source).map_err(|e| e.to_string())?;
    let before = input.metadata().map_err(|e| e.to_string())?;
    let limit = if source.extension().is_some_and(|s| s == "json") { JSON_LIMIT } else { LOG_LIMIT };
    if !before.is_file() || before.len() > limit { return Err("nonregular or oversized artifact".into()); }
    let identity = |m: &fs::Metadata| (m.dev(), m.ino(), m.len(), m.mtime(), m.mtime_nsec(), m.ctime(), m.ctime_nsec());
    let mut output = OpenOptions::new().write(true).create_new(true).open(destination).map_err(|e| e.to_string())?;
    let copied = io::copy(&mut (&mut input).take(before.len()), &mut output).map_err(|e| e.to_string())?;
    let after = input.metadata().map_err(|e| e.to_string())?;
    if copied != before.len() || identity(&before) != identity(&after) {
        return Err("artifact changed while copied".into());
    }
    Ok(())
}

fn merge(registry: &Path, inputs: &Path, output: &Path, shard_result: &str) -> Result<usize, String> {
    if shard_result != "success" { return Err("every shard must have succeeded".into()); }
    ordinary_directory(inputs)?;
    let checks: Vec<_> = xtask::checks::load(registry)?.into_iter().filter(|c| c.profile == "extended").collect();
    if !checks.iter().any(|c| c.id == "semantic-mutants") || checks.len() < 2 {
        return Err("semantic and remaining extended checks required".into());
    }
    let shard_names = BTreeSet::from(["pilot-receipts-semantic".to_owned(), "pilot-receipts-remaining".to_owned()]);
    let mut actual_names = BTreeSet::new();
    for entry in fs::read_dir(inputs).map_err(|e| e.to_string())? {
        let name = entry.map_err(|e| e.to_string())?.file_name().into_string().map_err(|_| "non-UTF8 shard name".to_owned())?;
        if !shard_names.contains(&name) || !actual_names.insert(name) {
            return Err("unexpected or duplicate shard directory".into());
        }
    }
    if actual_names != shard_names { return Err("missing or unexpected shard directory".into()); }
    let mut artifacts = Vec::new();
    for shard in &shard_names {
        let directory = inputs.join(shard);
        ordinary_directory(&directory)?;
        let expected: BTreeSet<_> = checks.iter().filter(|c| (c.id == "semantic-mutants") == (shard == "pilot-receipts-semantic")).map(|c| c.id.clone()).collect();
        let mut receipts = BTreeSet::new();
        let mut names = BTreeSet::new();
        for entry in fs::read_dir(&directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if !fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?.is_file() {
                return Err("ordinary shard artifact required".into());
            }
            let name = entry.file_name().into_string().map_err(|_| "non-UTF8 artifact name")?;
            if !expected.iter().any(|id| [format!("{id}.json"), format!("{id}.log"), format!("{id}.stdout.partial.log"), format!("{id}.stderr.partial.log")].contains(&name)) {
                return Err(format!("unexpected or incomplete artifact: {name}"));
            }
            if let Some(id) = name.strip_suffix(".json") { receipts.insert(id.to_owned()); }
            names.insert(name);
        }
        if receipts != expected { return Err("missing, duplicate or misplaced check receipt".into()); }
        for id in expected {
            for name in [format!("{id}.json"), format!("{id}.log")] {
                if !names.contains(&name) { return Err(format!("missing check artifact: {name}")); }
                artifacts.push((directory.join(&name), name));
            }
        }
    }
    fs::create_dir(output).map_err(|e| format!("exclusive output directory: {e}"))?;
    for (source, name) in artifacts { copy_bounded(&source, &output.join(name))?; }
    Ok(checks.len())
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let result = (|| {
        if args.len() != 4 || args[0] != "--shard-result" { return Err("usage: merge_verification_receipts --shard-result RESULT INPUT OUTPUT".to_owned()); }
        let storage = fs::canonicalize(std::env::var_os("FABRIC_STORAGE_ROOT").ok_or("explicit DATA storage required")?).map_err(|e| e.to_string())?;
        let inputs = PathBuf::from(&args[2]);
        let output = PathBuf::from(&args[3]);
        if !inputs.canonicalize().map_err(|e| e.to_string())?.starts_with(&storage)
            || !output.parent().ok_or("output parent required")?.canonicalize().map_err(|e| e.to_string())?.starts_with(&storage)
            || output.file_name().is_none() { return Err("DATA-only paths required".into()); }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().ok_or("workspace root required")?;
        merge(&root.join("xtask/checks.json"), &inputs, &output, args[1].to_str().ok_or("invalid shard result")?)
    })();
    match result {
        Ok(count) => println!("collected {count} checks; independent final validation still required"),
        Err(error) => { eprintln!("shard collection failed: {error}"); std::process::exit(1); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    const COMMIT: &str = "0123456789012345678901234567890123456789";

    fn fixture() -> PathBuf {
        let root = PathBuf::from(std::env::var_os("FABRIC_SCRATCH_ROOT").expect("contained DATA scratch")).join(format!("shard-collector-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&root).unwrap();
        let mut entries = Vec::new();
        for (shard, id) in [("semantic", "semantic-mutants"), ("remaining", "other-check")] {
            let directory = root.join("inputs").join(format!("pilot-receipts-{shard}"));
            fs::create_dir_all(&directory).unwrap();
            let log = b"finite collector fixture\n";
            fs::write(directory.join(format!("{id}.log")), log).unwrap();
            fs::write(directory.join(format!("{id}.json")), serde_json::to_vec(&json!({
                "receipt_version":1,"check_id":id,"candidate_commit":COMMIT,"worktree_dirty":false,
                "profile":"extended","command":["fixture-check"],"result":"passed","exit_code":0,
                "output_log":format!("{id}.log"),"output_sha256":xtask::sha256_hex(log),"duration_ms":1
            })).unwrap()).unwrap();
            entries.push(json!({"id":id,"profile":"extended","command":["fixture-check"],"scope":"fixture","expected":"pass","unchecked":"not actual execution"}));
        }
        fs::write(root.join("registry.json"), serde_json::to_vec(&json!({"checks":entries})).unwrap()).unwrap();
        root
    }

    fn collect(root: &Path, status: &str) -> Result<usize, String> {
        merge(&root.join("registry.json"), &root.join("inputs"), &root.join("combined"), status)
    }

    fn independent(root: &Path) -> bool {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let script = "import json,sys; from pathlib import Path; sys.path.insert(0,sys.argv[1]); import verify_receipts; verify_receipts.verify(json.loads(Path(sys.argv[2]).read_text()),'extended',sys.argv[3],Path(sys.argv[4]))";
        std::process::Command::new("python3").args(["-B", "-c", script]).arg(workspace.join("tools/ci")).arg(root.join("registry.json")).arg(COMMIT).arg(root.join("combined")).output().unwrap().status.success()
    }

    #[test]
    fn complete_bytes_reach_unchanged_independent_validator() {
        let root = fixture();
        assert_eq!(collect(&root, "success").unwrap(), 2);
        assert!(independent(&root));
        assert!(collect(&root, "success").is_err(), "must not overwrite old output");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_skipped_canceled_or_absent_shard_cannot_create_output() {
        let root = fixture();
        for status in ["failure", "failed", "cancelled", "skipped", ""] {
            assert!(collect(&root, status).is_err());
            assert!(!root.join("combined").exists());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_duplicate_running_and_symlink_inputs_are_rejected() {
        for defect in ["missing", "duplicate", "running", "symlink"] {
            let root = fixture();
            let remaining = root.join("inputs/pilot-receipts-remaining");
            match defect {
                "missing" => fs::remove_file(remaining.join("other-check.json")).unwrap(),
                "duplicate" => { fs::copy(root.join("inputs/pilot-receipts-semantic/semantic-mutants.json"), remaining.join("semantic-mutants.json")).unwrap(); },
                "running" => fs::write(remaining.join("current-check.json"), b"{}").unwrap(),
                "symlink" => { fs::remove_file(remaining.join("other-check.log")).unwrap(); std::os::unix::fs::symlink("/dev/null", remaining.join("other-check.log")).unwrap(); },
                _ => unreachable!(),
            }
            assert!(collect(&root, "success").is_err(), "accepted {defect}");
            assert!(!root.join("combined").exists());
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn stale_and_modified_evidence_still_fail_after_collection() {
        for defect in ["stale", "log"] {
            let root = fixture();
            let directory = root.join("inputs/pilot-receipts-remaining");
            if defect == "stale" {
                let path = directory.join("other-check.json");
                let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                value["candidate_commit"] = json!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
                fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
            } else { fs::write(directory.join("other-check.log"), b"changed").unwrap(); }
            assert_eq!(collect(&root, "success").unwrap(), 2);
            assert!(!independent(&root), "independent gate accepted {defect}");
            fs::remove_dir_all(root).unwrap();
        }
    }
}
