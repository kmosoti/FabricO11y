//! Validate complete verification receipts against the current registry.
//!
//! This checks evidence integrity and completeness. It does not establish that
//! a command ran or that the command's underlying property is adequate.

use crate::preflight::ArtifactStore;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

#[derive(Debug)]
struct Check {
    id: String,
    command: Vec<String>,
    duration_ms: u64,
}

fn require(condition: bool, message: &str) -> Result<(), String> {
    if condition { Ok(()) } else { Err(message.to_owned()) }
}

fn identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes[0].is_ascii_lowercase()
        && bytes.iter().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

fn registry_checks(registry: &Value, profile: &str) -> Result<Vec<Check>, String> {
    let checks = registry.get("checks").and_then(Value::as_array)
        .ok_or("registry checks must be a list")?;
    let mut seen = HashSet::new();
    let mut selected = Vec::new();
    for entry in checks {
        let object = entry.as_object().ok_or("invalid registry entry")?;
        let id = object.get("id").and_then(Value::as_str).ok_or("invalid registry ID")?;
        require(identifier(id) && seen.insert(id.to_owned()), "invalid or duplicate check ID")?;
        let args = object.get("command").and_then(Value::as_array).ok_or("invalid check command")?;
        require(!args.is_empty(), "invalid check command")?;
        let command = args.iter().map(|arg| arg.as_str().map(str::to_owned)
            .ok_or_else(|| format!("invalid check command: {id}")))
            .collect::<Result<Vec<_>, _>>()?;
        if object.get("profile").and_then(Value::as_str) == Some(profile) {
            selected.push(Check { id: id.to_owned(), command, duration_ms: 0 });
        }
    }
    require(!selected.is_empty(), &format!("profile has no checks: {profile}"))?;
    Ok(selected)
}

fn string_field<'a>(object: &'a serde_json::Map<String, Value>, key: &str) -> Result<&'a str, String> {
    object.get(key).and_then(Value::as_str).ok_or_else(|| format!("{key} must be a string"))
}

fn directory_identity(path: &Path) -> Result<(u64, u64, i64, i64, i64, i64), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    require(metadata.is_dir(), "receipt root is not an ordinary directory")?;
    Ok((metadata.dev(), metadata.ino(), metadata.mtime(), metadata.mtime_nsec(), metadata.ctime(), metadata.ctime_nsec()))
}

fn validate_receipt(
    receipt: &Value,
    check: &Check,
    profile: &str,
    commit: &str,
) -> Result<u64, String> {
    let object = receipt.as_object().ok_or_else(|| format!("invalid receipt: {}", check.id))?;
    let version = object.get("receipt_version").and_then(Value::as_u64);
    require(version == Some(1), &format!("invalid receipt version: {}", check.id))?;
    require(string_field(object, "check_id")? == check.id.as_str(), "receipt ID mismatch")?;
    require(string_field(object, "candidate_commit")? == commit, "stale receipt commit")?;
    require(object.get("worktree_dirty") == Some(&Value::Bool(false)), "dirty source receipt")?;
    require(string_field(object, "profile")? == profile, "receipt profile mismatch")?;
    let command = object.get("command").and_then(Value::as_array).ok_or("receipt command mismatch")?;
    require(command.len() == check.command.len()
        && command.iter().zip(&check.command).all(|(left, right)| left.as_str() == Some(right)),
        "receipt command mismatch")?;
    require(string_field(object, "result")? == "passed", "receipt did not pass")?;
    require(object.get("exit_code").and_then(Value::as_u64) == Some(0), "invalid exit status")?;
    let duration = object.get("duration_ms").and_then(Value::as_u64)
        .ok_or_else(|| format!("invalid duration: {}", check.id))?;
    let expected_log = format!("{}.log", check.id);
    require(string_field(object, "output_log")? == expected_log.as_str(), "output log name mismatch")?;
    let digest = string_field(object, "output_sha256")?;
    require(digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid output SHA-256")?;
    Ok(duration)
}

/// Validate one profile using strict bounded JSON and the shared descriptor
/// reader. `commit` is the caller's checked-out Git identity.
pub fn verify(root: &Path, profile: &str, commit: &str, receipts: &Path) -> Result<Value, String> {
    require(profile == "fast" || profile == "extended", "unsupported verification profile")?;
    require(commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "a complete checked-out commit identity is required")?;
    require(root.is_absolute() && receipts.is_absolute(), "absolute source and receipt roots required")?;

    let mut source = ArtifactStore::new(root)?;
    let mut checks = registry_checks(source.observe_json("xtask/checks.json")?, profile)?;
    let remaining_json = (64 * 1024 * 1024u64).checked_sub(source.json_bytes())
        .ok_or("registry exceeds retained JSON budget")?;
    let mut evidence = ArtifactStore::with_json_budget(receipts, remaining_json)?;

    let marker_directory_before = directory_identity(receipts)?;
    let marker = receipts.join("current-check.json");
    match fs::symlink_metadata(&marker) {
        Ok(_) => return Err("an interrupted or still-running check remains".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }

    let mut total = 0u64;
    let mut results = Vec::with_capacity(checks.len());
    for check in &mut checks {
        let receipt_path = format!("{}.json", check.id);
        let receipt = evidence.observe_json(&receipt_path)?;
        check.duration_ms = validate_receipt(receipt, check, profile, commit)?;

        let log_path = format!("{}.log", check.id);
        let pin = receipt.get("output_sha256").and_then(Value::as_str).ok_or("output SHA-256 missing")?.to_owned();
        evidence.read(&log_path, &pin, false)?;
        total = total.checked_add(check.duration_ms).ok_or("total check duration overflow")?;
        results.push(json!({"check_id": check.id.clone(), "duration_ms": check.duration_ms}));
    }
    require(directory_identity(receipts)? == marker_directory_before,
        "receipt directory changed during verification")?;
    source.recheck()?;
    evidence.recheck()?;
    results.sort_by(|left, right| {
        right["duration_ms"].as_u64().cmp(&left["duration_ms"].as_u64())
    });
    Ok(json!({
        "candidate_commit": commit,
        "profile": profile,
        "status": "passed",
        "check_count": results.len(),
        "total_check_ms": total,
        "checks_by_duration": results,
        "limits": "Receipt integrity and completeness, not a security certification."
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture { base: std::path::PathBuf, root: std::path::PathBuf, receipts: std::path::PathBuf }
    impl Drop for Fixture { fn drop(&mut self) {
        if std::thread::panicking() { eprintln!("failed receipt fixture retained: {}", self.base.display()); }
        else { let _ = fs::remove_dir_all(&self.base); }
    } }

    fn new_fixture() -> Fixture {
        let scratch = std::path::PathBuf::from(std::env::var_os("FABRIC_SCRATCH_ROOT").expect("DATA scratch"));
        let base = scratch.join(format!("receipt-unit-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&base).unwrap();
        let root = base.join("source");
        let receipts = base.join("receipts");
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("xtask")).unwrap();
        fs::create_dir(&receipts).unwrap();
        fs::write(root.join("xtask/checks.json"), br#"{"checks":[{"id":"fast-one","profile":"fast","command":["cargo","test"]},{"id":"extended-one","profile":"extended","command":["sh","-c","true"]},{"id":"extended-two","profile":"extended","command":["echo","ok"]}]}"#).unwrap();
        let log = b"synthetic fixture\n";
        fs::write(receipts.join("extended-one.log"), log).unwrap();
        let receipt = json!({
            "receipt_version": 1, "check_id": "extended-one", "candidate_commit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "worktree_dirty": false, "profile": "extended", "command": ["sh", "-c", "true"],
            "result": "passed", "exit_code": 0, "duration_ms": 12, "output_log": "extended-one.log",
            "output_sha256": crate::sha256_hex(log), "descriptive_extra": {"kept": true}
        });
        fs::write(receipts.join("extended-one.json"), serde_json::to_vec(&receipt).unwrap()).unwrap();
        let second_log = b"synthetic second fixture\n";
        fs::write(receipts.join("extended-two.log"), second_log).unwrap();
        let second_receipt = json!({
            "receipt_version": 1, "check_id": "extended-two", "candidate_commit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "worktree_dirty": false, "profile": "extended", "command": ["echo", "ok"],
            "result": "passed", "exit_code": 0, "duration_ms": 12, "output_log": "extended-two.log",
            "output_sha256": crate::sha256_hex(second_log)
        });
        fs::write(receipts.join("extended-two.json"), serde_json::to_vec(&second_receipt).unwrap()).unwrap();
        Fixture { base, root, receipts }
    }

    fn verify_fixture(fixture: &Fixture) -> Result<Value, String> {
        verify(&fixture.root, "extended", &"a".repeat(40), &fixture.receipts)
    }

    #[test]
    fn complete_synthetic_receipt_set_passes_with_python_shape() {
        let fixture = new_fixture();
        let report = verify_fixture(&fixture).unwrap();
        assert_eq!(report["status"], "passed");
        assert_eq!(report["check_count"], 2);
        assert_eq!(report["total_check_ms"], 24);
        assert_eq!(report["checks_by_duration"][0]["check_id"], "extended-one");
        assert_eq!(report["checks_by_duration"][1]["check_id"], "extended-two");
    }

    #[test]
    fn common_missing_stale_dirty_and_marker_controls_fail_closed() {
        let fixture = new_fixture();
        fs::remove_file(fixture.receipts.join("extended-one.json")).unwrap();
        assert!(verify_fixture(&fixture).is_err());

        let fixture = new_fixture();
        let path = fixture.receipts.join("extended-one.json");
        let mut receipt: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        receipt["candidate_commit"] = json!("b".repeat(40));
        fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
        assert!(verify_fixture(&fixture).is_err());

        let fixture = new_fixture();
        let path = fixture.receipts.join("extended-one.json");
        let mut receipt: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        receipt["worktree_dirty"] = json!(true);
        fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
        assert!(verify_fixture(&fixture).is_err());

        let fixture = new_fixture();
        std::os::unix::fs::symlink(fixture.base.join("does-not-exist"), fixture.receipts.join("current-check.json")).unwrap();
        assert!(verify_fixture(&fixture).is_err());
    }

    #[test]
    fn stronger_json_representation_probes_are_rejected() {
        for (from, to) in [
            ("\"receipt_version\":1,", "\"receipt_version\":1,\"receipt_version\":1,"),
            ("\"receipt_version\":1", "\"receipt_version\":true"),
            ("\"receipt_version\":1", "\"receipt_version\":1.0"),
            ("\"worktree_dirty\":false", "\"worktree_dirty\":0"),
        ] {
            let fixture = new_fixture();
            let path = fixture.receipts.join("extended-one.json");
            let raw = fs::read_to_string(&path).unwrap();
            let changed = raw.replacen(from, to, 1);
            assert_ne!(changed, raw);
            fs::write(&path, changed).unwrap();
            assert!(verify_fixture(&fixture).is_err());
        }
    }
}
