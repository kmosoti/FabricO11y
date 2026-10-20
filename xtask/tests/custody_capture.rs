//! DATA-owned capture-manifest controls for the existing structural cutoff CLI.
//! These inputs are synthetic and do not attest a guest or replace native joins.
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const CANDIDATE: &str = "0123456789abcdef0123456789abcdef01234567";
const INSPECT: &[u8] =
    include_bytes!("../../docs/experiments/formal/fixtures/native-inspect-success.txt");
static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    storage: PathBuf,
    root: PathBuf,
    manifest: Value,
}

impl Fixture {
    fn new(phase: &str) -> Self {
        let storage = PathBuf::from(std::env::var_os("FABRIC_STORAGE_ROOT").expect("DATA root"));
        let scratch = PathBuf::from(std::env::var_os("FABRIC_SCRATCH_ROOT").expect("DATA scratch"));
        assert!(scratch.starts_with(&storage), "scratch must be under DATA");
        let root = scratch.join(format!(
            "custody-capture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let phase_root = root.join("phase").join(phase);
        fs::create_dir_all(&phase_root).expect("create owned capture fixture");
        let mut capture: Value = serde_json::from_str(include_str!(
            "../../docs/experiments/formal/fixtures/native-cutoff-projection-input.json"
        ))
        .unwrap();
        capture.as_object_mut().unwrap().remove("classification");
        capture["phase"] = json!(phase);
        capture["relative_phase_root"] = json!(relative(&storage, &phase_root));
        if phase == "reboot" {
            capture["observed_events"][1]["message"] =
                json!("config revision=4 paused=true interval_s=15");
        }
        // Model the native archive's full observation shape. The Rust reader must
        // select the four snapshot fields and the two source identity fields.
        capture["stopped_snapshot"]["ack"] = json!(1);
        capture["stopped_snapshot"]["files"] = json!({"identity": {"sha256": "c".repeat(64)}});
        capture["stopped_snapshot"]["diagnostics"] =
            json!({"spindle.log": {"sha256": "d".repeat(64)}});
        capture["stopped_snapshot"]["sources"][0]["payload"] = json!({"raw": "retained"});
        capture["stopped_snapshot"]["sources"][0]["rows"] = json!([{"body": "captured row"}]);
        let inspection_path = phase_root.join("inspection.txt");
        fs::write(&inspection_path, INSPECT).expect("write literal inspection bytes");
        let manifest = json!({
            "version": 1,
            "candidate": CANDIDATE,
            "phase": phase,
            "prior_ack": capture["prior_ack"],
            "relative_phase_root": capture["relative_phase_root"],
            "commands": store(&storage, &phase_root, "commands.json", &capture["commands"]),
            "observed_events": store(&storage, &phase_root, "observed-events.json", &capture["observed_events"]),
            "process_observations": store(&storage, &phase_root, "process-observations.json", &capture["process_observations"]),
            "stopped_snapshot": store(&storage, &phase_root, "stopped-snapshot.json", &capture["stopped_snapshot"])
        });
        Self {
            storage,
            root,
            manifest,
        }
    }

    fn run(&self, manifest: &Value, candidate: &str, wrong_pin: bool, mixed: bool) -> Output {
        let bytes = serde_json::to_vec(manifest).unwrap();
        let path = self.root.join("capture-manifest.json");
        fs::write(&path, &bytes).expect("write capture manifest");
        let relative_manifest = relative(&self.storage, &path);
        let digest = if wrong_pin {
            "0".repeat(64)
        } else {
            xtask::sha256_hex(&bytes)
        };
        let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
        command.args([
            "custody-cutoff",
            "--root",
            self.storage.to_str().unwrap(),
            "--capture-manifest",
            &relative_manifest,
            "--manifest-sha256",
            &digest,
            "--candidate",
            candidate,
        ]);
        if mixed {
            command.args(["--plan", &relative_manifest, "--plan-sha256", &digest]);
        }
        command.output().expect("run actual xtask capture reader")
    }

    fn write_member(&mut self, key: &str, name: &str, value: &Value) {
        let path = self
            .root
            .join("phase")
            .join(self.manifest["phase"].as_str().unwrap())
            .join(name);
        let bytes = serde_json::to_vec(value).unwrap();
        fs::write(&path, &bytes).expect("replace one capture member");
        self.manifest[key]["sha256"] = json!(xtask::sha256_hex(&bytes));
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove owned DATA fixture");
    }
}

fn store(storage: &Path, phase_root: &Path, name: &str, value: &Value) -> Value {
    let path = phase_root.join(name);
    let bytes = serde_json::to_vec(value).unwrap();
    fs::write(&path, &bytes).expect("write captured JSON member");
    json!({"path": relative(storage, &path), "sha256": xtask::sha256_hex(&bytes)})
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .expect("fixture is DATA-owned")
        .to_str()
        .unwrap()
        .replace('\\', "/")
}

fn invalid(output: Output, context: &str) {
    assert_eq!(
        output.status.code(),
        Some(2),
        "{context}: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let report: Value = serde_json::from_slice(&output.stdout).expect("structured refusal");
    assert_eq!(report["status"], "invalid", "{context}");
}

#[test]
fn capture_manifest_projects_actual_drain_and_reboot_inputs() {
    for phase in ["drain", "reboot"] {
        let fixture = Fixture::new(phase);
        let output = fixture.run(&fixture.manifest, CANDIDATE, false, false);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{phase}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let report: Value = serde_json::from_slice(&output.stdout).expect("structural report");
        assert_eq!(report["status"], "structural-cutoff");
        assert_eq!(report["cutoff"], 1);
        assert!(
            report["scope"]
                .as_str()
                .unwrap()
                .contains("Caller-normalized observations only")
        );
    }
}

#[test]
fn capture_manifest_rejects_candidate_pin_unknown_keys_and_mixed_mode() {
    let fixture = Fixture::new("drain");
    invalid(
        fixture.run(&fixture.manifest, "b".repeat(40).as_str(), false, false),
        "candidate",
    );
    invalid(
        fixture.run(&fixture.manifest, CANDIDATE, true, false),
        "manifest digest",
    );
    let mut unknown = fixture.manifest.clone();
    unknown["unexpected"] = json!(1);
    invalid(
        fixture.run(&unknown, CANDIDATE, false, false),
        "unknown manifest field",
    );
    invalid(
        fixture.run(&fixture.manifest, CANDIDATE, false, true),
        "mixed mode",
    );
}

#[test]
fn capture_manifest_rejects_escape_crossroot_and_substituted_members() {
    let fixture = Fixture::new("drain");
    let mut escape = fixture.manifest.clone();
    escape["commands"]["path"] = json!("../outside.json");
    invalid(
        fixture.run(&escape, CANDIDATE, false, false),
        "phase escape",
    );
    let mut crossroot = fixture.manifest.clone();
    crossroot["stopped_snapshot"]["path"] = json!("/etc/passwd");
    invalid(
        fixture.run(&crossroot, CANDIDATE, false, false),
        "absolute member path",
    );

    let mut changed = fixture.manifest.clone();
    let commands_path = fixture
        .storage
        .join(changed["commands"]["path"].as_str().unwrap());
    let mut commands: Value = serde_json::from_slice(&fs::read(commands_path).unwrap()).unwrap();
    commands[1]["stdout_artifact"]["sha256"] = json!("0".repeat(64));
    let bytes = serde_json::to_vec(&commands).unwrap();
    let path = fixture
        .storage
        .join(changed["commands"]["path"].as_str().unwrap());
    fs::write(path, &bytes).unwrap();
    changed["commands"]["sha256"] = json!(xtask::sha256_hex(&bytes));
    invalid(
        fixture.run(&changed, CANDIDATE, false, false),
        "substituted source pin",
    );

    let body = Fixture::new("drain");
    fs::write(
        body.root.join("phase/drain/inspection.txt"),
        b"substituted body",
    )
    .expect("substitute retained raw inspection");
    invalid(
        body.run(&body.manifest, CANDIDATE, false, false),
        "substituted raw body",
    );
}

#[test]
fn capture_manifest_rejects_source_ack_order_and_oversized_streams() {
    let mut source = Fixture::new("drain");
    let mut snapshot: Value = serde_json::from_slice(
        &fs::read(
            source.storage.join(
                source.manifest["stopped_snapshot"]["path"]
                    .as_str()
                    .unwrap(),
            ),
        )
        .unwrap(),
    )
    .unwrap();
    snapshot["sources"][0]["sha256"] = json!("b".repeat(64));
    source.write_member("stopped_snapshot", "stopped-snapshot.json", &snapshot);
    invalid(
        source.run(&source.manifest, CANDIDATE, false, false),
        "source/ACK mismatch",
    );

    let mut missing = Fixture::new("drain");
    let mut events: Value = serde_json::from_slice(
        &fs::read(
            missing.storage.join(
                missing.manifest["observed_events"]["path"]
                    .as_str()
                    .unwrap(),
            ),
        )
        .unwrap(),
    )
    .unwrap();
    events.as_array_mut().unwrap().pop();
    missing.write_member("observed_events", "observed-events.json", &events);
    invalid(
        missing.run(&missing.manifest, CANDIDATE, false, false),
        "missing exact ACK",
    );

    let mut reordered = Fixture::new("drain");
    let mut events: Value = serde_json::from_slice(
        &fs::read(
            reordered.storage.join(
                reordered.manifest["observed_events"]["path"]
                    .as_str()
                    .unwrap(),
            ),
        )
        .unwrap(),
    )
    .unwrap();
    events.as_array_mut().unwrap().reverse();
    reordered.write_member("observed_events", "observed-events.json", &events);
    invalid(
        reordered.run(&reordered.manifest, CANDIDATE, false, false),
        "ordered stream",
    );

    let mut bounded = Fixture::new("drain");
    let events = Value::Array((0..=65536).map(|_| json!({})).collect());
    bounded.write_member("observed_events", "observed-events.json", &events);
    invalid(
        bounded.run(&bounded.manifest, CANDIDATE, false, false),
        "event bound",
    );
}
