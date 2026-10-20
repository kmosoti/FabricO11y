//! The fixture owns expected receipt decisions; this file only constructs its
//! synthetic inputs and invokes the real `xtask verify-receipts` executable.
//! Descriptor replacement and mutation during reads remain covered by the
//! existing `preflight::files::during_read_replacement_and_mutation_are_refused`
//! test; this suite covers deterministic CLI-level receipt cases.
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
const CONTRACT: &str = include_str!("fixtures/receipt_contract_cases.json");

struct Fixture {
    base: PathBuf,
    receipts: PathBuf,
    profile: String,
    commit: String,
    checks: Vec<Value>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "failed receipt CLI fixture retained: {}",
                self.base.display()
            );
        } else {
            let _ = fs::remove_dir_all(&self.base);
        }
    }
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?}: {}",
        arguments,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn new_fixture(profile: &str) -> Fixture {
    let scratch =
        PathBuf::from(std::env::var_os("FABRIC_SCRATCH_ROOT").expect("contained DATA scratch"));
    let base = scratch.join(format!(
        "receipt-cli-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&base).unwrap();
    let receipts = base.join("receipts");
    fs::create_dir(&receipts).unwrap();
    let root = workspace();
    let registry: Value =
        serde_json::from_slice(&fs::read(root.join("xtask/checks.json")).unwrap()).unwrap();
    let checks: Vec<Value> = registry["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|check| check["profile"] == profile)
        .cloned()
        .collect();
    assert!(
        !checks.is_empty(),
        "fixture profile must have selected checks"
    );
    let commit = git(&root, &["rev-parse", "HEAD"]);
    for check in &checks {
        write_check(&receipts, check, profile, &commit, 1);
    }
    Fixture {
        base,
        receipts,
        profile: profile.to_owned(),
        commit,
        checks,
    }
}

fn write_check(receipts: &Path, check: &Value, profile: &str, commit: &str, duration: u64) {
    let id = check["id"].as_str().unwrap();
    let log_name = format!("{id}.log");
    let log = format!("synthetic receipt contract fixture: {id}\n").into_bytes();
    fs::write(receipts.join(&log_name), &log).unwrap();
    let receipt = json!({
        "receipt_version": 1,
        "check_id": id,
        "candidate_commit": commit,
        "worktree_dirty": false,
        "profile": profile,
        "command": check["command"],
        "result": "passed",
        "exit_code": 0,
        "duration_ms": duration,
        "output_log": log_name,
        "output_sha256": xtask::sha256_hex(&log),
        "descriptive_extra": {"kept": true}
    });
    fs::write(
        receipts.join(format!("{id}.json")),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
}

fn first_receipt(fixture: &Fixture) -> PathBuf {
    fixture.receipts.join(format!(
        "{}.json",
        fixture.checks[0]["id"].as_str().unwrap()
    ))
}

fn mutate_first_receipt(fixture: &Fixture, field: &str, value: Value) {
    let path = first_receipt(fixture);
    let mut receipt: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    receipt[field] = value;
    fs::write(path, serde_json::to_vec(&receipt).unwrap()).unwrap();
}

fn cli(fixture: &Fixture) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args([
            "verify-receipts",
            "--profile",
            &fixture.profile,
            "--receipts",
            fixture.receipts.to_str().unwrap(),
        ])
        .env(
            "FABRIC_STORAGE_ROOT",
            std::env::var_os("FABRIC_STORAGE_ROOT").expect("DATA storage"),
        )
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap()
}

fn run_expected(fixture: &Fixture, expected_code: i64, case: &str) {
    let output = cli(fixture);
    assert_eq!(
        output.status.code(),
        Some(expected_code as i32),
        "case {case}: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if expected_code == 0 {
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["status"], "passed", "case {case}");
        assert_eq!(report["candidate_commit"], fixture.commit, "case {case}");
        assert_eq!(report["check_count"], fixture.checks.len(), "case {case}");
        assert_eq!(
            report["total_check_ms"],
            fixture.checks.len(),
            "case {case}"
        );
    }
}

fn case(id: &str) -> (String, i64) {
    let fixture: Value = serde_json::from_str(CONTRACT).unwrap();
    let selected = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == id)
        .unwrap_or_else(|| panic!("contract case missing: {id}"));
    (
        selected["mutation"].as_str().unwrap().to_owned(),
        selected["exit_code"].as_i64().unwrap(),
    )
}

fn status_case(id: &str, mutation: impl FnOnce(&Fixture)) {
    let (name, expected) = case(id);
    let profile = if name == "pad-twenty-valid-receipts-past-64mib-json-budget"
        || name == "set-durations-to-u64-max-and-one"
    {
        "fast"
    } else {
        "extended"
    };
    let fixture = new_fixture(profile);
    mutation(&fixture);
    run_expected(&fixture, expected, id);
}

#[test]
fn fixture_expected_outcomes_drive_real_cli_positive_and_refusal_cases() {
    let parsed: Value = serde_json::from_str(CONTRACT).unwrap();
    assert_eq!(
        parsed["expected_outcomes_are_independent_of_implementation"],
        true
    );
    assert!(parsed["retired_python_test_mapping"].is_object());

    for id in [
        "positive-fast",
        "positive-extended",
        "descriptive-extra-field",
    ] {
        let profile = if id == "positive-fast" {
            "fast"
        } else {
            "extended"
        };
        let fixture = new_fixture(profile);
        run_expected(&fixture, case(id).1, id);
    }

    status_case("missing-receipt", |f| {
        fs::remove_file(first_receipt(f)).unwrap()
    });
    status_case("missing-log", |f| {
        let id = f.checks[0]["id"].as_str().unwrap();
        fs::remove_file(f.receipts.join(format!("{id}.log"))).unwrap();
    });
    status_case("symlink-receipt", |f| {
        let id = f.checks[0]["id"].as_str().unwrap();
        let other = f.checks[1]["id"].as_str().unwrap();
        fs::remove_file(first_receipt(f)).unwrap();
        std::os::unix::fs::symlink(
            format!("{other}.json"),
            f.receipts.join(format!("{id}.json")),
        )
        .unwrap();
    });
    status_case("symlink-log", |f| {
        let id = f.checks[0]["id"].as_str().unwrap();
        let other = f.checks[1]["id"].as_str().unwrap();
        fs::remove_file(f.receipts.join(format!("{id}.log"))).unwrap();
        std::os::unix::fs::symlink(format!("{other}.log"), f.receipts.join(format!("{id}.log")))
            .unwrap();
    });
    status_case("running-marker", |f| {
        fs::write(f.receipts.join("current-check.json"), b"{}\n").unwrap()
    });
    status_case("dangling-marker-symlink", |f| {
        std::os::unix::fs::symlink(
            "missing-marker-target",
            f.receipts.join("current-check.json"),
        )
        .unwrap()
    });
    status_case("stale-commit", |f| {
        mutate_first_receipt(f, "candidate_commit", json!("0".repeat(40)))
    });
    status_case("dirty-flag", |f| {
        mutate_first_receipt(f, "worktree_dirty", json!(true))
    });
    status_case("wrong-command", |f| {
        mutate_first_receipt(f, "command", json!(["not-the-registered-command"]))
    });
    status_case("wrong-id", |f| {
        mutate_first_receipt(f, "check_id", json!("wrong-check"))
    });
    status_case("wrong-profile", |f| {
        mutate_first_receipt(f, "profile", json!("fast"))
    });
    status_case("wrong-result", |f| {
        mutate_first_receipt(f, "result", json!("failed"))
    });
    status_case("wrong-log-name", |f| {
        mutate_first_receipt(f, "output_log", json!("other.log"))
    });
    status_case("wrong-version", |f| {
        mutate_first_receipt(f, "receipt_version", json!(2))
    });
    status_case("nonzero-exit", |f| {
        mutate_first_receipt(f, "exit_code", json!(1))
    });
    status_case("nonzero-exit-three", |f| {
        mutate_first_receipt(f, "exit_code", json!(3))
    });
    status_case("unavailable-result", |f| {
        mutate_first_receipt(f, "result", json!("environment-unavailable"));
        mutate_first_receipt(f, "exit_code", Value::Null);
    });
    status_case("null-exit", |f| {
        mutate_first_receipt(f, "exit_code", Value::Null)
    });
    status_case("boolean-exit", |f| {
        mutate_first_receipt(f, "exit_code", json!(false))
    });
    status_case("float-exit", |f| {
        mutate_first_receipt(f, "exit_code", json!(0.0))
    });
    status_case("negative-duration", |f| {
        mutate_first_receipt(f, "duration_ms", json!(-1))
    });
    status_case("boolean-duration", |f| {
        mutate_first_receipt(f, "duration_ms", json!(true))
    });
    status_case("float-duration", |f| {
        mutate_first_receipt(f, "duration_ms", json!(1.5))
    });
    status_case("modified-log", |f| {
        let id = f.checks[0]["id"].as_str().unwrap();
        fs::write(f.receipts.join(format!("{id}.log")), b"modified bytes\n").unwrap();
    });
    status_case("duplicate-result-key", |f| {
        let path = first_receipt(f);
        let raw = fs::read_to_string(&path).unwrap();
        let changed = raw.replacen(
            "\"result\":\"passed\"",
            "\"result\":\"failed\",\"result\":\"passed\"",
            1,
        );
        assert_ne!(raw, changed);
        fs::write(path, changed).unwrap();
    });
    status_case("boolean-version", |f| {
        mutate_first_receipt(f, "receipt_version", json!(true))
    });
    status_case("float-version", |f| {
        mutate_first_receipt(f, "receipt_version", json!(1.0))
    });
    status_case("numeric-dirty-alias", |f| {
        mutate_first_receipt(f, "worktree_dirty", json!(0))
    });
    status_case("nonregular-receipt", |f| {
        let path = first_receipt(f);
        fs::remove_file(&path).unwrap();
        fs::create_dir(path).unwrap();
    });
    status_case("nonregular-log", |f| {
        let id = f.checks[0]["id"].as_str().unwrap();
        let path = f.receipts.join(format!("{id}.log"));
        fs::remove_file(&path).unwrap();
        fs::create_dir(path).unwrap();
    });
    status_case("oversized-receipt", |f| {
        let path = first_receipt(f);
        fs::write(&path, vec![b'x'; 4 * 1024 * 1024 + 1]).unwrap();
    });
    status_case("aggregate-budget", |f| {
        assert_eq!(f.checks.len(), 20, "fast profile budget fixture count");
        let padding = "x".repeat(3_600_000);
        for check in &f.checks {
            let id = check["id"].as_str().unwrap();
            let path = f.receipts.join(format!("{id}.json"));
            let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            value["contract_padding"] = json!(padding);
            let bytes = serde_json::to_vec(&value).unwrap();
            assert!(
                bytes.len() < 4 * 1024 * 1024,
                "individual receipt remains bounded"
            );
            fs::write(path, bytes).unwrap();
        }
    });
    status_case("duration-sum-overflow", |f| {
        mutate_first_receipt(f, "duration_ms", json!(u64::MAX));
        mutate_receipt_for_id(
            f,
            f.checks[1]["id"].as_str().unwrap(),
            "duration_ms",
            json!(1),
        );
    });
}

fn mutate_receipt_for_id(fixture: &Fixture, id: &str, field: &str, value: Value) {
    let path = fixture.receipts.join(format!("{id}.json"));
    let mut receipt: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    receipt[field] = value;
    fs::write(path, serde_json::to_vec(&receipt).unwrap()).unwrap();
}

#[test]
fn cli_rejects_a_real_dirty_git_checkout() {
    let (mutation, expected) = case("dirty-checkout");
    assert_eq!(mutation, "run-cli-against-dirty-git-worktree");
    for dirty_kind in ["tracked", "untracked"] {
        let fixture = new_fixture("extended");
        let fake_checkout = fixture.base.join(format!("dirty-checkout-{dirty_kind}"));
        fs::create_dir(&fake_checkout).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(&fake_checkout)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["config", "user.name", "Receipt Fixture"])
                .current_dir(&fake_checkout)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["config", "user.email", "receipt-fixture@example.invalid"])
                .current_dir(&fake_checkout)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["config", "core.hooksPath", "/dev/null"])
                .current_dir(&fake_checkout)
                .status()
                .unwrap()
                .success()
        );
        fs::write(fake_checkout.join("tracked"), b"clean base\n").unwrap();
        assert!(
            Command::new("git")
                .args(["add", "tracked"])
                .current_dir(&fake_checkout)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args([
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "--quiet",
                    "-m",
                    "clean fixture"
                ])
                .current_dir(&fake_checkout)
                .status()
                .unwrap()
                .success()
        );
        if dirty_kind == "tracked" {
            fs::write(fake_checkout.join("tracked"), b"modified after receipt\n").unwrap();
        } else {
            fs::write(
                fake_checkout.join("untracked"),
                b"untracked after receipt\n",
            )
            .unwrap();
        }
        let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args([
                "verify-receipts",
                "--profile",
                "extended",
                "--receipts",
                fixture.receipts.to_str().unwrap(),
            ])
            .env(
                "FABRIC_STORAGE_ROOT",
                std::env::var_os("FABRIC_STORAGE_ROOT").unwrap(),
            )
            .env("GIT_DIR", fake_checkout.join(".git"))
            .env("GIT_WORK_TREE", &fake_checkout)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(expected as i32),
            "{dirty_kind}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("candidate worktree changed"),
            "{dirty_kind}"
        );
    }
}

#[test]
fn rust_registry_reader_refuses_invalid_commands_duplicate_ids_and_empty_profiles() {
    for (case_id, registry, expected_error) in [
        (
            "invalid-registry-command",
            json!({"checks":[{"id":"bad-command","profile":"extended","command":[7]}]}),
            "invalid check command",
        ),
        (
            "invalid-registry-id",
            json!({"checks":[{"id":"../escape","profile":"extended","command":["check"]}]}),
            "invalid or duplicate check ID",
        ),
        (
            "duplicate-registry-id",
            json!({"checks":[{"id":"duplicate","profile":"extended","command":["first"]},{"id":"duplicate","profile":"extended","command":["second"]}]}),
            "invalid or duplicate check ID",
        ),
        (
            "empty-profile",
            json!({"checks":[{"id":"other-profile","profile":"fast","command":["check"]}]}),
            "profile has no checks: extended",
        ),
    ] {
        let (mutation, expected) = case(case_id);
        assert_eq!(expected, 1);
        let fixture = new_fixture("extended");
        let source = fixture.base.join(format!("source-{case_id}"));
        fs::create_dir_all(source.join("xtask")).unwrap();
        fs::write(
            source.join("xtask/checks.json"),
            serde_json::to_vec(&registry).unwrap(),
        )
        .unwrap();
        let error =
            xtask::receipts::verify(&source, "extended", &fixture.commit, &fixture.receipts)
                .expect_err(&format!("case {case_id} unexpectedly accepted; {mutation}"));
        assert!(
            error.contains(expected_error),
            "case {case_id} reached the wrong refusal: {error}"
        );
    }
}
