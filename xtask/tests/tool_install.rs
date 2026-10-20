//! Synthetic CLI controls for the bounded Cargo tool installer.

use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    root: PathBuf,
    storage: PathBuf,
    scratch: PathBuf,
    fake_bin: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let outer_scratch = PathBuf::from(
            std::env::var_os("FABRIC_SCRATCH_ROOT").expect("resource launcher DATA scratch"),
        );
        let root = outer_scratch.join(format!(
            "tool-install-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("create test fixture under DATA scratch");
        let storage = root.join("storage");
        let scratch = storage.join("scratch");
        let fake_bin = root.join("fake-bin");
        fs::create_dir_all(&scratch).expect("create fixture DATA scratch");
        fs::create_dir(&fake_bin).expect("create fake tool directory");
        let fake_cargo = fake_bin.join("cargo");
        fs::write(
            &fake_cargo,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$FAKE_ARGS\"\nprintf 'stdout-marker\\n'\nprintf 'stderr-marker\\n' >&2\nprintf '%s\\n' \"${CARGO_HOME:-}\" > \"$FAKE_CARGO_HOME\"\nexit \"${FAKE_CARGO_EXIT:-0}\"\n",
        )
        .expect("write fake cargo executable");
        fs::set_permissions(&fake_cargo, fs::Permissions::from_mode(0o700))
            .expect("make fake cargo executable");
        Self {
            root,
            storage,
            scratch,
            fake_bin,
        }
    }

    fn invoke(&self, crate_name: &str, version: &str, cargo_exit: &str) -> Output {
        let args = self.root.join("fake-args.txt");
        let home = self.root.join("fake-cargo-home.txt");
        let path = format!("{}:/usr/bin:/bin", self.fake_bin.display());
        Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(["install-tool", crate_name, version])
            .env("PATH", path)
            .env("CARGO_HOME", self.root.join("cargo-home"))
            .env("FAKE_ARGS", args)
            .env("FAKE_CARGO_HOME", home)
            .env("FAKE_CARGO_EXIT", cargo_exit)
            .env("FABRIC_STORAGE_ROOT", &self.storage)
            .env("FABRIC_SCRATCH_ROOT", &self.scratch)
            .output()
            .expect("launch xtask installer command")
    }

    fn receipt(&self, crate_name: &str) -> Value {
        let path = self
            .storage
            .join("results/ci-tool-install")
            .join(format!("{crate_name}.json"));
        serde_json::from_slice(&fs::read(path).expect("read install receipt"))
            .expect("parse install receipt")
    }

    fn fake_args(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("fake-args.txt"))
            .expect("fake Cargo was launched")
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "failed tool installer fixture retained: {}",
                self.root.display()
            );
        } else {
            fs::remove_dir_all(&self.root).expect("remove installer fixture");
        }
    }
}

#[test]
fn success_preserves_cargo_argv_log_receipt_and_scratch_cleanup() {
    let fixture = Fixture::new();
    let output = fixture.invoke("cargo-deny", "0.20.2", "0");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let receipt = fixture.receipt("cargo-deny");
    let target = PathBuf::from(receipt["target"].as_str().expect("target path"));
    let target_text = target.to_string_lossy().into_owned();
    assert!(target.starts_with(&fixture.scratch));
    assert!(!target.exists());
    assert_eq!(receipt["crate"], "cargo-deny");
    assert_eq!(receipt["version"], "0.20.2");
    assert_eq!(receipt["exit"], 0);
    assert!(receipt["cleanup_absent"].as_bool().unwrap());
    assert_eq!(
        receipt["command"],
        serde_json::json!([
            "cargo",
            "install",
            "cargo-deny",
            "--version",
            "0.20.2",
            "--locked",
            "--target-dir",
            target_text
        ])
    );
    assert_eq!(
        fixture.fake_args(),
        vec![
            "install".to_owned(),
            "cargo-deny".to_owned(),
            "--version".to_owned(),
            "0.20.2".to_owned(),
            "--locked".to_owned(),
            "--target-dir".to_owned(),
            target_text.clone()
        ]
    );
    let log = fs::read_to_string(
        fixture
            .storage
            .join("results/ci-tool-install/cargo-deny.log"),
    )
    .expect("install output log");
    assert!(log.contains("stdout-marker"));
    assert!(log.contains("stderr-marker"));
    assert!(receipt["disk_after"]["total"].is_u64());
    assert!(receipt["disk_after"]["used"].is_u64());
    assert!(receipt["disk_after"]["free"].is_u64());
    assert_eq!(
        fs::read_to_string(fixture.root.join("fake-cargo-home.txt"))
            .unwrap()
            .trim(),
        fixture.root.join("cargo-home").to_str().unwrap()
    );
}

#[test]
fn cargo_failure_keeps_log_and_failure_receipt_after_cleanup() {
    let fixture = Fixture::new();
    let output = fixture.invoke("cargo-fuzz", "0.13.2", "23");
    assert_eq!(output.status.code(), Some(1));

    let receipt = fixture.receipt("cargo-fuzz");
    let target = PathBuf::from(receipt["target"].as_str().unwrap());
    assert_eq!(receipt["exit"], 23);
    assert!(receipt["cleanup_absent"].as_bool().unwrap());
    assert!(!target.exists());
    assert!(receipt["error"].as_str().unwrap().contains("retained log"));
    assert!(
        fixture
            .storage
            .join("results/ci-tool-install/cargo-fuzz.log")
            .is_file()
    );
}

#[test]
fn cargo_signal_is_preserved_in_the_failure_receipt() {
    let fixture = Fixture::new();
    fs::write(fixture.fake_bin.join("cargo"), "#!/bin/sh\nkill -TERM $$\n").unwrap();
    let output = fixture.invoke("cargo-deny", "0.20.2", "0");
    assert!(!output.status.success());
    let receipt = fixture.receipt("cargo-deny");
    assert_eq!(receipt["exit"], -15);
    assert_eq!(receipt["cleanup_absent"], true);
    assert!(receipt["error"].is_string());
}

#[test]
fn replaced_target_is_retained_instead_of_removed() {
    let fixture = Fixture::new();
    fs::write(
        fixture.fake_bin.join("cargo"),
        "#!/bin/sh\nmv \"$7\" \"$7.original\"\nmkdir \"$7\"\nprintf keep > \"$7/sentinel\"\n",
    )
    .unwrap();
    assert!(!fixture.invoke("cargo-deny", "0.20.2", "0").status.success());
    let receipt = fixture.receipt("cargo-deny");
    assert_eq!(receipt["exit"], 0);
    assert_eq!(receipt["cleanup_absent"], false);
    assert!(
        receipt["error"]
            .as_str()
            .unwrap()
            .contains("identity changed")
    );
    let target = Path::new(receipt["target"].as_str().unwrap());
    assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"keep");
}

#[test]
fn missing_data_roots_refuse_before_starting_cargo() {
    for variable in ["FABRIC_STORAGE_ROOT", "FABRIC_SCRATCH_ROOT"] {
        let fixture = Fixture::new();
        let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(["install-tool", "cargo-deny", "0.20.2"])
            .env("PATH", &fixture.fake_bin)
            .env("FABRIC_STORAGE_ROOT", &fixture.storage)
            .env("FABRIC_SCRATCH_ROOT", &fixture.scratch)
            .env_remove(variable)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains(variable));
        assert!(!fixture.root.join("fake-args.txt").exists());
        assert!(!fixture.storage.join("results/ci-tool-install").exists());
    }
}

#[test]
fn invalid_names_and_versions_are_rejected_before_cargo() {
    for (crate_name, version) in [
        ("../cargo-deny", "0.20.2"),
        ("cargo-deny", ""),
        ("cargo-deny", "0..2"),
        ("cargo-deny", "0.2/../../x"),
    ] {
        let fixture = Fixture::new();
        let output = fixture.invoke(crate_name, version, "0");
        assert_eq!(output.status.code(), Some(2));
        assert!(!fixture.root.join("fake-args.txt").exists());
        assert!(!fixture.storage.join("results/ci-tool-install").exists());
    }
}

#[test]
fn scratch_outside_storage_is_refused_before_cargo() {
    let fixture = Fixture::new();
    let outside = fixture.root.join("outside-scratch");
    fs::create_dir(&outside).expect("create outside scratch fixture");
    let args = fixture.root.join("fake-args.txt");
    let home = fixture.root.join("fake-cargo-home.txt");
    let path = format!("{}:/usr/bin:/bin", fixture.fake_bin.display());
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["install-tool", "cargo-deny", "0.20.2"])
        .env("PATH", path)
        .env("FAKE_ARGS", args)
        .env("FAKE_CARGO_HOME", home)
        .env("FABRIC_STORAGE_ROOT", &fixture.storage)
        .env("FABRIC_SCRATCH_ROOT", &outside)
        .output()
        .expect("launch xtask installer command");
    assert_eq!(output.status.code(), Some(2));
    assert!(!fixture.root.join("fake-args.txt").exists());
}

#[test]
fn unwritable_receipt_cannot_turn_install_into_success() {
    let fixture = Fixture::new();
    let output_dir = fixture.storage.join("results/ci-tool-install");
    fs::create_dir_all(&output_dir).expect("create receipt directory");
    fs::create_dir(output_dir.join("cargo-deny.json")).expect("block receipt path");
    let output = fixture.invoke("cargo-deny", "0.20.2", "0");
    assert_eq!(output.status.code(), Some(1));
    assert!(output_dir.join("cargo-deny.log").is_file());
    assert!(fixture.fake_args().iter().any(|arg| arg == "install"));
}

#[test]
fn required_workflow_pins_and_resource_launcher_are_present() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let calls = [
        ("rust.yml", "cargo-deny", "0.20.2"),
        ("verification.yml", "cargo-deny", "0.20.2"),
        ("verification.yml", "cargo-mutants", "27.1.0"),
        ("verification.yml", "cargo-llvm-cov", "0.9.1"),
        ("verification.yml", "cargo-fuzz", "0.13.2"),
        ("verification.yml", "kani-verifier", "0.68.0"),
        (
            "verification-concurrency-experiment.yml",
            "cargo-deny",
            "0.20.2",
        ),
        (
            "verification-concurrency-experiment.yml",
            "cargo-mutants",
            "27.1.0",
        ),
        (
            "verification-concurrency-experiment.yml",
            "cargo-llvm-cov",
            "0.9.1",
        ),
        (
            "verification-concurrency-experiment.yml",
            "cargo-fuzz",
            "0.13.2",
        ),
        (
            "verification-concurrency-experiment.yml",
            "kani-verifier",
            "0.68.0",
        ),
    ];
    for (workflow, crate_name, version) in calls {
        let source = fs::read_to_string(root.join(".github/workflows").join(workflow)).unwrap();
        let old = format!(
            "python3 -B tools/resource_group.py -- python3 -B tools/ci/install-cargo-tool.py {crate_name} {version}"
        );
        let new = format!(
            "python3 -B tools/resource_group.py -- cargo xtask install-tool {crate_name} {version}"
        );
        assert_ne!(
            source.contains(&old),
            source.contains(&new),
            "{workflow}: {crate_name}"
        );
        assert!(source.contains("${{ env.FABRIC_STORAGE_ROOT }}/results/ci-tool-install/"));
    }
}
