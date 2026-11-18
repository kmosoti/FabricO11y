use std::{path::Path, process::Command};

#[test]
fn settings_emit_toml_and_report_daemon_sources() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let profile = manifest.join("../config/fabric.toml");
    let absent = format!("/data/fabric-settings-test-{}", std::process::id());
    let mut command = Command::new(env!("CARGO_BIN_EXE_fabric"));
    command
        .arg("--config")
        .arg(profile)
        .args(["settings", "--interval-ms", "10"])
        .env("FABRIC_DATA_DIR", &absent)
        .env("XDG_CONFIG_HOME", &absent)
        .env_remove("FABRIC_TMP_DIR")
        .env_remove("SPINDLE_SOCKET")
        .env_remove("XDG_RUNTIME_DIR");

    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: toml::Table = toml::from_str(&String::from_utf8(result.stdout).unwrap()).unwrap();
    assert_eq!(report["version"].as_integer(), Some(1));
    assert_eq!(report["daemon"]["interval_ms"].as_integer(), Some(250));
    assert_eq!(report["daemon"]["data_dir"].as_str(), Some(absent.as_str()));
    assert_eq!(report["sources"]["data_dir"].as_str(), Some("environment"));
    assert_eq!(
        report["sources"]["interval_ms"].as_str(),
        Some("command_line")
    );
    assert_eq!(
        report["sources"]["max_disk_bytes"].as_str(),
        Some("profile")
    );
    assert_eq!(report["spindle"]["status"].as_str(), Some("missing"));

    let result = command.arg("--json").output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["command"], "settings");
    assert_eq!(report["data"]["daemon"]["interval_ms"], 250);
    let data = &report["data"];
    assert_eq!(data["sources"]["ring_budget_bytes"], "automatic");
    let expected_bytes = data["ring_budget"]["cgroup_limit_bytes"]
        .as_u64()
        .map_or(32 * 1024 * 1024, |limit| {
            ((limit - limit.div_ceil(4)) / 8).clamp(4096, 32 * 1024 * 1024)
        });
    assert_eq!(data["ring_budget"]["automatic_bytes"], expected_bytes);
    assert_eq!(data["daemon"]["ring_budget_bytes"], expected_bytes);

    let override_result = command
        .args(["--ring-budget-bytes", "4096"])
        .output()
        .unwrap();
    assert!(override_result.status.success());
    let override_report: serde_json::Value =
        serde_json::from_slice(&override_result.stdout).unwrap();
    assert_eq!(
        override_report["data"]["sources"]["ring_budget_bytes"],
        "command_line"
    );
    assert_eq!(override_report["data"]["daemon"]["ring_budget_bytes"], 4096);
    assert!(!Path::new(&absent).exists());
}
