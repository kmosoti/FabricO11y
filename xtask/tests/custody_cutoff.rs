//! CLI-level controls for the normalized cutoff adapter. These tests exercise
//! the executable boundary, but do not replace native binding or custody oracles.
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use xtask::sha256_hex;

const CANDIDATE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NODE: &str = "5e97d1a36ba9e6f6b8a1ed085fac10b7";
const BOOT: &str = "11111111111111111111111111111111";
const INVOCATION: &str = "22222222222222222222222222222222";
const SOURCE_DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const INSPECT: &str =
    include_str!("../../docs/experiments/formal/fixtures/native-inspect-success.txt");

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    storage: PathBuf,
    directory: PathBuf,
    plan_relative: String,
    plan: Value,
    inspection: String,
    stopped_inspection: String,
}

impl Fixture {
    fn new() -> Self {
        let storage =
            PathBuf::from(std::env::var_os("FABRIC_STORAGE_ROOT").expect("DATA storage required"));
        let scratch =
            PathBuf::from(std::env::var_os("FABRIC_SCRATCH_ROOT").expect("DATA scratch required"));
        assert!(
            scratch.starts_with(&storage),
            "scratch must remain under DATA storage"
        );
        let directory = scratch.join(format!(
            "custody-cutoff-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).expect("create owned DATA fixture");
        let inspection_path = directory.join("inspect.txt");
        fs::write(&inspection_path, INSPECT).expect("write captured inspection fixture");
        let input_relative = relative(&storage, &inspection_path);
        let stopped_path = directory.join("inspect-stopped.txt");
        fs::write(&stopped_path, INSPECT).expect("write separate stopped inspection fixture");
        let stopped_relative = relative(&storage, &stopped_path);
        let plan = json!({
            "version": 1,
            "candidate": CANDIDATE,
            "node_id": NODE,
            "generation": 1,
            "incarnation": {
                "boot": BOOT,
                "pid": 42,
                "start_ticks": 10,
                "invocation": INVOCATION
            },
            "revision": 2,
            "window": {
                "unit": "fabrico11y-node.service",
                "pid": 42,
                "boot": BOOT,
                "start_ns": 0,
                "end_ns": 20
            },
            "sources": [{"sequence": 1, "sha256": SOURCE_DIGEST}],
            "events": [{
                "unit": "fabrico11y-node.service",
                "pid": 42,
                "boot": BOOT,
                "at_ns": 15,
                "sequence": 1,
                "sha256": SOURCE_DIGEST,
                "through": 1
            }],
            "trace": [
                {"kind":"batch", "sender":sender(), "at_ns":1, "sequence":1},
                {"kind":"pause-request", "sender":sender(), "started_ns":5, "finished_ns":8, "revision":2},
                {"kind":"pause-applied", "sender":sender(), "at_ns":7, "revision":2},
                {"kind":"inspection", "sender":sender(), "started_ns":10, "finished_ns":11,
                    "revision":2, "paused":true, "stopped":false,
                    "input":{"path":input_relative, "sha256":sha256_hex(INSPECT.as_bytes())}},
                {"kind":"inspection", "sender":sender(), "started_ns":12, "finished_ns":13,
                    "revision":2, "paused":true, "stopped":false,
                    "input":{"path":input_relative, "sha256":sha256_hex(INSPECT.as_bytes())}},
                {"kind":"stop", "started_ns":14, "finished_ns":16},
                {"kind":"inspection", "sender":sender(), "started_ns":17, "finished_ns":18,
                    "revision":2, "paused":true, "stopped":true,
                    "input":{"path":stopped_relative, "sha256":sha256_hex(INSPECT.as_bytes())}}
            ]
        });
        let plan_relative = relative(&storage, &directory.join("plan.json"));
        Self {
            storage,
            directory,
            plan_relative,
            plan,
            inspection: INSPECT.to_owned(),
            stopped_inspection: INSPECT.to_owned(),
        }
    }

    fn run(&self, plan: &Value, pin_override: Option<&str>, candidate: &str) -> Output {
        let bytes = serde_json::to_vec(plan).expect("serialize fixture plan");
        fs::write(self.directory.join("plan.json"), &bytes).expect("write fixture plan");
        let pin = pin_override
            .map(str::to_owned)
            .unwrap_or_else(|| sha256_hex(&bytes));
        Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args([
                "custody-cutoff",
                "--root",
                self.storage.to_str().unwrap(),
                "--plan",
                &self.plan_relative,
                "--plan-sha256",
                &pin,
                "--candidate",
                candidate,
            ])
            .output()
            .expect("run actual xtask CLI")
    }

    fn assert_invalid(&self, plan: &Value, expected: &str) {
        let output = self.run(plan, None, CANDIDATE);
        assert_eq!(
            output.status.code(),
            Some(2),
            "unexpected status: {}",
            stdout(&output)
        );
        let report: Value = serde_json::from_slice(&output.stdout).expect("JSON invalid report");
        assert_eq!(report["status"], "invalid", "{}", stdout(&output));
        let error = report["error"].as_str().expect("structured error");
        assert!(
            error.contains(expected),
            "expected {expected:?}, got {error:?}"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.directory).expect("remove owned DATA fixture");
    }
}

fn sender() -> Value {
    json!({"boot":BOOT, "pid":42, "start_ticks":10, "invocation":INVOCATION})
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .expect("fixture path under DATA root")
        .to_str()
        .expect("UTF-8 DATA path")
        .replace('\\', "/")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn actual_cli_accepts_valid_plan_and_reuses_pinned_raw_inspection() {
    let fixture = Fixture::new();
    let output = fixture.run(&fixture.plan, None, CANDIDATE);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    let report: Value = serde_json::from_slice(&output.stdout).expect("JSON structural report");
    assert_eq!(report["status"], "structural-cutoff");
    assert_eq!(report["cutoff"], 1);
    assert_eq!(
        report["bytes_hashed"],
        (fixture.inspection.len() + fixture.stopped_inspection.len()) as u64
            + fs::metadata(fixture.directory.join("plan.json"))
                .unwrap()
                .len()
    );
    assert!(
        report["scope"]
            .as_str()
            .unwrap()
            .contains("Caller-normalized observations only")
    );
    assert!(
        !report["scope"]
            .as_str()
            .unwrap()
            .contains("custody attested")
    );
}

#[test]
fn actual_cli_rejects_candidate_pin_missing_raw_and_substituted_raw() {
    let fixture = Fixture::new();
    let output = fixture.run(
        &fixture.plan,
        None,
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).contains("plan version/candidate mismatch"));

    let output = fixture.run(&fixture.plan, Some(&"0".repeat(64)), CANDIDATE);
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).contains("artifact hash mismatch"));

    let raw_path = fixture.directory.join("inspect.txt");
    fs::remove_file(&raw_path).unwrap();
    fixture.assert_invalid(&fixture.plan, "No such file");
    fs::write(
        &raw_path,
        fixture.inspection.replace("batches=1", "batches=2"),
    )
    .unwrap();
    fixture.assert_invalid(&fixture.plan, "artifact hash mismatch");
}

#[test]
fn actual_cli_rejects_cutoff_identity_timing_and_lifecycle_counterexamples() {
    let fixture = Fixture::new();
    let mut cases: Vec<(&str, Value, &str)> = Vec::new();

    let mut plan = fixture.plan.clone();
    plan["generation"] = json!(2);
    cases.push((
        "node-generation drift",
        plan,
        "inspection node/generation mismatch",
    ));

    let mut plan = fixture.plan.clone();
    plan["trace"][3]["started_ns"] = json!(7);
    cases.push((
        "overlapping inspection",
        plan,
        "overlapping or reversed command intervals",
    ));

    let mut plan = fixture.plan.clone();
    plan["trace"][6]["started_ns"] = json!(15);
    cases.push((
        "inspection overlaps stop completion",
        plan,
        "overlapping or reversed command intervals",
    ));

    let mut plan = fixture.plan.clone();
    plan["events"][0]["pid"] = json!(43);
    cases.push(("wrong ACK sender", plan, "SenderOrWindow"));

    let mut plan = fixture.plan.clone();
    plan["events"] = json!([]);
    cases.push(("missing exact ACK", plan, "Missing(1)"));

    let mut plan = fixture.plan.clone();
    plan["trace"][0]["sequence"] = json!(9);
    cases.push(("commit beyond cutoff one", plan, "CommitBeyondPrefix"));

    let mut plan = fixture.plan.clone();
    plan["trace"].as_array_mut().unwrap().pop();
    plan["trace"].as_array_mut().unwrap().remove(5);
    cases.push(("missing stop", plan, "IncompleteTrace"));

    let mut plan = fixture.plan.clone();
    plan["trace"][0]["unexpected"] = json!(true);
    cases.push(("unknown trace field", plan, "unexpected or missing fields"));

    for (label, plan, expected) in cases {
        let output = fixture.run(&plan, None, CANDIDATE);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{label}: {}",
            stdout(&output)
        );
        let report: Value = serde_json::from_slice(&output.stdout).expect("JSON invalid report");
        assert_eq!(report["status"], "invalid", "{label}");
        assert!(
            report["error"].as_str().unwrap().contains(expected),
            "{label}: {}",
            stdout(&output)
        );
    }
}

#[test]
fn actual_cli_preserves_health_refusal_and_rejects_ack_order_and_raw_bounds() {
    let fixture = Fixture::new();
    let mut recovery = fixture.plan.clone();
    let running_path = fixture.directory.join("inspect.txt");
    let stopped_path = fixture.directory.join("inspect-stopped.txt");
    let recovery_body = fixture
        .inspection
        .replace("recovery_required=false", "recovery_required=true");
    fs::write(&running_path, &recovery_body).unwrap();
    fs::write(&stopped_path, &recovery_body).unwrap();
    let digest = sha256_hex(recovery_body.as_bytes());
    for event in recovery["trace"].as_array_mut().unwrap() {
        if event["kind"] == "inspection" {
            event["input"]["sha256"] = json!(digest);
        }
    }
    fixture.assert_invalid(&recovery, "RecoveryRequired");

    fs::write(&running_path, &fixture.inspection).unwrap();
    fs::write(&stopped_path, &fixture.stopped_inspection).unwrap();
    let mut unordered = fixture.plan.clone();
    unordered["events"].as_array_mut().unwrap().push(json!({
        "unit":"fabrico11y-node.service", "pid":42, "boot":BOOT, "at_ns":16,
        "sequence":0, "sha256":SOURCE_DIGEST, "through":0
    }));
    fixture.assert_invalid(&unordered, "UnorderedAcks");

    let stopped_drift = fixture
        .stopped_inspection
        .replace("next_sequence=2", "next_sequence=3");
    fs::write(&stopped_path, &stopped_drift).unwrap();
    let mut changed_stopped = fixture.plan.clone();
    changed_stopped["trace"][6]["input"]["sha256"] = json!(sha256_hex(stopped_drift.as_bytes()));
    fixture.assert_invalid(&changed_stopped, "StoppedSnapshotMismatch");
    fs::write(&stopped_path, &fixture.stopped_inspection).unwrap();

    let oversized = format!("{}{}", fixture.inspection, " ".repeat(4097));
    fs::write(&running_path, &oversized).unwrap();
    let mut bounded = fixture.plan.clone();
    let digest = sha256_hex(oversized.as_bytes());
    for event in bounded["trace"].as_array_mut().unwrap() {
        if event["kind"] == "inspection"
            && event["input"]["path"] == relative(&fixture.storage, &running_path)
        {
            event["input"]["sha256"] = json!(digest);
        }
    }
    fixture.assert_invalid(&bounded, "TooLarge");
}
