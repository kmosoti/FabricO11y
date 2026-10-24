//! Captured metadata controls: projection plus the actual public
//! xtask custody consumer. This is not an acceptance or native-attestation test.
//! Run under the required DATA resource launcher.
use xtask::custody::projection;

use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const CANDIDATE: &str = "0123456789abcdef0123456789abcdef01234567";
static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

fn input() -> Value {
    serde_json::from_str(include_str!(
        "../../docs/experiments/formal/fixtures/native-cutoff-projection-input.json"
    ))
    .unwrap()
}

fn project_case(value: &Value) -> Result<Value, String> {
    projection::project(projection::Inputs {
        commands: value["commands"].as_array().unwrap(),
        observed_events: value["observed_events"].as_array().unwrap(),
        process_observations: value["process_observations"].as_array().unwrap(),
        stopped_snapshot: &value["stopped_snapshot"],
        phase: value["phase"].as_str().unwrap(),
        prior_ack: value["prior_ack"].as_u64().unwrap(),
        candidate: value["candidate"].as_str().unwrap(),
        relative_phase_root: value["relative_phase_root"].as_str().unwrap(),
    })
}

fn scratch_root() -> PathBuf {
    let base = PathBuf::from(
        std::env::var("FABRIC_SCRATCH_ROOT").expect("contained scratch root required"),
    );
    assert!(base.starts_with(std::env::var_os("FABRIC_STORAGE_ROOT").expect("DATA storage")));
    let path = base.join(format!(
        "projection-consumer-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).expect("unique owned root");
    path
}

fn consumer(plan: &Value, inspection: &[u8]) -> Result<Value, String> {
    let root = scratch_root();
    let result = (|| {
        let relative_artifact = plan["trace"]
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["kind"] == "inspection")
            .unwrap()["input"]["path"]
            .as_str()
            .unwrap();
        let artifact = root.join(relative_artifact);
        fs::create_dir_all(artifact.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::write(&artifact, inspection).map_err(|e| e.to_string())?;
        let plan_bytes = serde_json::to_vec(plan).map_err(|e| e.to_string())?;
        let digest = xtask::sha256_hex(&plan_bytes);
        fs::write(root.join("plan.json"), &plan_bytes).map_err(|e| e.to_string())?;
        xtask::custody::verify_plan(&root, "plan.json", &digest, CANDIDATE)
    })();
    fs::remove_dir_all(&root).expect("remove only this test's unique root");
    result
}

fn inspection_bytes() -> Vec<u8> {
    include_bytes!("../../docs/experiments/formal/fixtures/native-inspect-success.txt").to_vec()
}

#[test]
fn drain_projection_reaches_actual_consumer() {
    let plan = project_case(&input()).expect("synthetic drain projection");
    let result = consumer(&plan, &inspection_bytes()).expect("actual custody consumer");
    assert_eq!(result["status"], "structural-cutoff");
    assert_eq!(result["cutoff"], 1);
}

#[test]
fn reboot_projection_reaches_actual_consumer() {
    let mut value = input();
    value["phase"] = json!("reboot");
    value["relative_phase_root"] = json!("phase/reboot");
    value["observed_events"][1]["message"] = json!("config revision=4 paused=true interval_s=15");
    let plan = project_case(&value).expect("synthetic reboot projection");
    assert_eq!(plan["revision"], 4);
    let result = consumer(&plan, &inspection_bytes()).expect("actual custody consumer");
    assert_eq!(result["cutoff"], 1);
}

#[test]
fn actual_consumer_rejects_extra_unit_in_strict_incarnation() {
    let mut plan = project_case(&input()).expect("baseline projection");
    plan["incarnation"]["unit"] = json!("fabrico11y-node.service");
    let error = consumer(&plan, &inspection_bytes()).unwrap_err();
    assert!(error.contains("unexpected or missing fields"), "{error}");
}

#[test]
fn projector_refuses_wrong_sender_process_and_missing_observations() {
    let mut value = input();
    value["process_observations"][1]["calibration"]["start_ticks"] = json!(11);
    assert!(project_case(&value).is_err(), "process incarnation drift");

    let mut value = input();
    value["observed_events"][0]["pid"] = json!(43);
    assert!(project_case(&value).is_err(), "wrong journal sender");

    let mut value = input();
    value["commands"].as_array_mut().unwrap().remove(2);
    assert!(
        project_case(&value).is_err(),
        "second drain inspection missing"
    );

    let mut value = input();
    value["commands"][2]
        .as_object_mut()
        .unwrap()
        .remove("stdout_artifact");
    assert!(
        project_case(&value).is_err(),
        "old capture missing inspection artifact"
    );

    let mut value = input();
    value["stopped_snapshot"]["candidate_process"]
        .as_object_mut()
        .unwrap()
        .remove("end_ns");
    assert!(
        project_case(&value).is_err(),
        "final process window missing"
    );
}

#[test]
fn projector_refuses_malformed_recognized_events_and_missing_ack() {
    let mut value = input();
    value["observed_events"][0]["message"] = json!(
        "batch=1x metrics=47 logs=4 gaps=0 spool_bytes=5666 log_backlog_bytes=0 acked_through=1"
    );
    assert!(project_case(&value).is_err(), "recognized malformed batch");

    let mut value = input();
    value["observed_events"][2]["message"] =
        json!("delivery sequence=1 sha256=xyz status=ack committed_through=1 elapsed_us=10");
    assert!(
        project_case(&value).is_err(),
        "recognized malformed delivery"
    );

    let mut value = input();
    value["observed_events"].as_array_mut().unwrap().pop();
    let plan = project_case(&value).expect("projection leaves cutoff decision to consumer");
    assert!(
        consumer(&plan, &inspection_bytes()).is_err(),
        "consumer requires actual ACK evidence"
    );
}

#[test]
fn retry_diagnostics_preserve_real_ack_and_never_supply_ack_credit() {
    for status in ["no_response", "unavailable"] {
        let mut value = input();
        let mut retry = value["observed_events"][2].clone();
        retry["message"] = json!(format!(
            "delivery sequence=1 sha256={} status={status} elapsed_us=10",
            "a".repeat(64)
        ));
        value["observed_events"]
            .as_array_mut()
            .unwrap()
            .insert(2, retry.clone());
        let plan = project_case(&value).expect("existing sender retry grammar");
        let result = consumer(&plan, &inspection_bytes()).expect("later actual ACK still required");
        assert_eq!(result["cutoff"], 1);
        value["observed_events"].as_array_mut().unwrap().pop();
        let plan = project_case(&value).expect("retry carries no ACK credit");
        assert!(
            consumer(&plan, &inspection_bytes()).is_err(),
            "retry replaced real ACK"
        );
    }
}

#[test]
fn retry_diagnostic_grammar_and_sender_identity_remain_strict() {
    let prefix = format!("delivery sequence=1 sha256={} ", "a".repeat(64));
    for suffix in [
        "status=unknown elapsed_us=10",
        "status=no_response committed_through=1 elapsed_us=10",
        "status=no_response elapsed_us=-1",
        "status=no_response elapsed_us=18446744073709551616",
        "status=no_response elapsed_us=10 extra=1",
    ] {
        let mut value = input();
        value["observed_events"][2]["message"] = json!(format!("{prefix}{suffix}"));
        assert!(project_case(&value).is_err(), "{suffix}");
    }
    let mut value = input();
    value["observed_events"][2]["message"] =
        json!("delivery sequence=1 sha256=xyz status=no_response elapsed_us=10");
    assert!(project_case(&value).is_err(), "malformed retry digest");
    let mut value = input();
    value["observed_events"][2]["message"] =
        json!(format!("{prefix}status=no_response elapsed_us=10"));
    value["observed_events"][2]["pid"] = json!(999999);
    assert!(project_case(&value).is_err(), "retry sender must bind");
}

#[test]
fn actual_consumer_rejects_recovery_required_inspection() {
    let mut plan = project_case(&input()).expect("baseline projection");
    let mut raw = String::from_utf8(inspection_bytes()).unwrap();
    raw = raw.replace("recovery_required=false", "recovery_required=true");
    let digest = xtask::sha256_hex(raw.as_bytes());
    for event in plan["trace"].as_array_mut().unwrap() {
        if event["kind"] == "inspection" {
            event["input"]["sha256"] = json!(digest);
        }
    }
    assert!(
        consumer(&plan, raw.as_bytes()).is_err(),
        "recovery-required state must refuse cutoff"
    );
}

#[test]
fn process_sample_must_follow_actual_applied_marker() {
    // Origin: projection-red-01 accepted marker215 after the last process sample210.
    let mut value = input();
    value["observed_events"][1]["at_ns"] = json!(215);
    // Baseline01 admits this pre-marker sample; semantic02a must reject it.
    assert!(project_case(&value).is_err(), "post-marker sample required");
}

#[test]
fn captured_metadata_cli_composes_the_owned_reader_and_cutoff() {
    let storage = PathBuf::from(std::env::var_os("FABRIC_STORAGE_ROOT").expect("DATA storage"));
    let root = scratch_root();
    let artifact = root.join("phase/drain/inspection.txt");
    fs::create_dir_all(artifact.parent().unwrap()).unwrap();
    let relative = root.strip_prefix(&storage).unwrap();
    let mut capture = input();
    capture.as_object_mut().unwrap().remove("classification");
    capture["version"] = json!(2);
    capture["relative_phase_root"] = json!(relative.join("phase/drain").to_str().unwrap());
    let plan_path = root.join("capture-plan.json");
    let relative_plan = plan_path.strip_prefix(&storage).unwrap();
    let original_inspection = inspection_bytes();
    for case in [
        "valid",
        "extra-field",
        "missing-field",
        "wrong-candidate",
        "pre-marker-sample",
        "wrong-source-digest",
        "substituted-body",
    ] {
        let mut value = capture.clone();
        match case {
            "extra-field" => value["unexpected"] = json!(1),
            "missing-field" => {
                value.as_object_mut().unwrap().remove("commands");
            }
            "wrong-candidate" => value["candidate"] = json!("b".repeat(40)),
            "pre-marker-sample" => value["observed_events"][1]["at_ns"] = json!(215),
            "wrong-source-digest" => {
                value["stopped_snapshot"]["sources"][0]["sha256"] = json!("b".repeat(64))
            }
            _ => {}
        }
        fs::write(
            &artifact,
            if case == "substituted-body" {
                b"substituted".as_slice()
            } else {
                &original_inspection
            },
        )
        .unwrap();
        let bytes = serde_json::to_vec(&value).unwrap();
        fs::write(&plan_path, &bytes).unwrap();
        let pin = xtask::sha256_hex(&bytes);
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args([
                "custody-cutoff",
                "--root",
                storage.to_str().unwrap(),
                "--plan",
                relative_plan.to_str().unwrap(),
                "--plan-sha256",
                &pin,
                "--candidate",
                CANDIDATE,
            ])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(if case == "valid" { 0 } else { 2 }),
            "{case}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["status"],
            if case == "valid" {
                "structural-cutoff"
            } else {
                "invalid"
            },
            "{case}"
        );
    }
    fs::remove_dir_all(root).unwrap();
}
