//! Exercise the cache restore policy with the pinned Cargo and an offline registry dependency.
#![cfg(target_os = "linux")]

use serde_json::Value;
use std::{
    fs::{self, File},
    path::Path,
    process::{Command, Output},
    time::{Duration, SystemTime},
};

const ACTION: &str = include_str!("../../.github/actions/restore-ci-cache/action.yml");
const WORKSPACE: &str = "[workspace]\nmembers = ['app', 'payload']\nresolver = '3'\n";
const BINARY: &str = "cache-fixture";

fn cargo(root: &Path) -> Command {
    let mut command = Command::new("cargo");
    command
        .arg("+1.100.0")
        .current_dir(root)
        .env("CARGO_TARGET_DIR", root.join("target"))
        .env("CARGO_BUILD_BUILD_DIR", root.join("target"))
        .env("CARGO_INCREMENTAL", "0");
    let cache_home = root.join("cache-home/.cargo");
    if cache_home.join("config.toml").is_file() {
        command.env("CARGO_HOME", cache_home);
    }
    command
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn workspace(root: &Path) {
    for directory in ["app/src", "payload/src"] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    fs::write(root.join("Cargo.toml"), WORKSPACE).unwrap();
    fs::write(root.join(".gitignore"), "/target/\n").unwrap();
    fs::write(
        root.join("app/Cargo.toml"),
        "[package]\nname = 'cache-fixture'\nversion = '0.1.0'\nedition = '2024'\n\
         [dependencies]\npayload = { path = '../payload' }\nitoa = '=1.0.18'\n",
    )
    .unwrap();
    fs::write(
        root.join("payload/Cargo.toml"),
        "[package]\nname = 'payload'\nversion = '0.1.0'\nedition = '2024'\n",
    )
    .unwrap();
    fs::write(
        root.join("app/src/main.rs"),
        r#"fn main() { println!("{} {}", payload::TAG, itoa::Buffer::new().format(42)); }"#,
    )
    .unwrap();
    fs::write(
        root.join("payload/src/lib.rs"),
        "pub const TAG: &str = \"A\";\n",
    )
    .unwrap();
    success(
        cargo(root)
            .args(["generate-lockfile", "--offline"])
            .output()
            .unwrap(),
    );
}

fn build(root: &Path, release: bool) -> Vec<Value> {
    let mut build = cargo(root);
    build.args([
        "build",
        "--locked",
        "--offline",
        "--bins",
        "--examples",
        "--message-format=json",
    ]);
    if release {
        build.arg("--release");
    }
    success(build.output().unwrap())
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|message| message["reason"] == "compiler-artifact")
        .collect()
}

fn fresh(artifacts: &[Value], target: &str) -> bool {
    artifacts
        .iter()
        .find(|artifact| artifact["target"]["name"] == target)
        .and_then(|artifact| artifact["fresh"].as_bool())
        .unwrap_or_else(|| panic!("missing Cargo artifact for {target}"))
}

fn binary_output(root: &Path, release: bool) -> String {
    let directory = if release { "release" } else { "debug" };
    success(
        Command::new(root.join("target").join(directory).join(BINARY))
            .output()
            .unwrap(),
    )
}

fn step_script(marker: &str) -> String {
    ACTION
        .split_once(marker)
        .unwrap()
        .1
        .split_once("      run: |\n")
        .unwrap()
        .1
        .lines()
        .take_while(|line| line.is_empty() || line.starts_with("        "))
        .map(|line| line.strip_prefix("        ").unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn cleanup(marker: &str, root: &Path) {
    success(
        Command::new("bash")
            .args(["-c", &step_script(marker)])
            .current_dir(root)
            .env("GITHUB_WORKSPACE", root)
            .env("CACHE_HOME", root.join("cache-home"))
            .output()
            .unwrap(),
    );
}

#[test]
fn compiled_miss_removes_old_targets_and_keeps_dependency_sources() {
    let scratch = xtask::evidence::non_tmp_tempdir().unwrap();
    let root = scratch.path();
    workspace(root);
    let cache_home = root.join("cache-home");
    let vendor = cache_home.join(".cargo/registry/vendor");
    let source_config = success(
        cargo(root)
            .args(["vendor", "--offline", "--locked"])
            .arg(&vendor)
            .output()
            .unwrap(),
    );
    fs::write(cache_home.join(".cargo/config.toml"), source_config).unwrap();
    let old_targets = [
        "app/src/bin/removed_bin.rs",
        "app/examples/removed_example.rs",
    ];
    for target in old_targets {
        let path = root.join(target);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, r#"fn main() { println!("removed source"); }"#).unwrap();
    }
    let source = root.join("payload/src/lib.rs");
    let archive_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    File::open(&source)
        .unwrap()
        .set_modified(archive_time)
        .unwrap();
    for release in [false, true] {
        assert!(!fresh(&build(root, release), "itoa"));
        assert_eq!(binary_output(root, release), "A 42\n");
    }

    // Emulate a restored target from an older source, including artifacts that
    // Cargo 1.100 workspace cleanup cannot enumerate after target removal.
    let saved_target = root.join("archived-target");
    fs::rename(root.join("target"), &saved_target).unwrap();
    for target in old_targets {
        fs::remove_file(root.join(target)).unwrap();
    }
    fs::write(&source, "pub const TAG: &str = \"B\";\n").unwrap();
    File::open(&source)
        .unwrap()
        .set_modified(archive_time)
        .unwrap();
    fs::rename(saved_target, root.join("target")).unwrap();
    for profile in ["debug", "release"] {
        for former in ["removed_bin", "examples/removed_example"] {
            let binary = root.join("target").join(profile).join(former);
            assert_eq!(
                success(Command::new(binary).output().unwrap()),
                "removed source\n"
            );
        }
    }
    for release in [false, true] {
        assert!(fresh(&build(root, release), "payload"));
        assert_eq!(binary_output(root, release), "A 42\n");
    }
    cleanup("    - name: Discard Cargo artifacts", root);
    assert!(!root.join("target").exists());
    assert!(vendor.join("itoa/src/lib.rs").is_file());
    for release in [false, true] {
        let rebuilt = build(root, release);
        assert!(!fresh(&rebuilt, "payload"));
        assert!(!fresh(&rebuilt, BINARY));
        assert!(!fresh(&rebuilt, "itoa"));
        assert_eq!(binary_output(root, release), "B 42\n");
    }

    let restored = [".cargo/registry", ".cargo/git/db"];
    let installed = [
        ".cargo/bin",
        ".rustup/toolchains",
        ".local/bin",
        ".local/share/fabrico11y/schema-tools",
        ".local/share/fabrico11y/engine-tools",
        ".local/share/fabrico11y/proof-tools",
    ];
    for directory in restored.into_iter().chain(installed) {
        fs::create_dir_all(cache_home.join(directory)).unwrap();
        fs::write(cache_home.join(directory).join("witness"), b"owned fixture").unwrap();
    }
    cleanup(
        "    - name: Discard incomplete dependency source restores",
        root,
    );
    for directory in restored {
        assert!(!cache_home.join(directory).exists());
    }
    for directory in installed {
        assert!(cache_home.join(directory).join("witness").is_file());
    }
    let fuzz = cache_home.join(".cache/fabrico11y-fuzz-target");
    fs::create_dir_all(&fuzz).unwrap();
    fs::write(fuzz.join("stale-fuzzer"), b"cache witness").unwrap();
    cleanup("    - name: Discard fuzz artifacts", root);
    assert!(!fuzz.exists());
    assert_eq!(binary_output(root, false), "B 42\n");
}

#[test]
fn only_dependency_sources_have_cross_revision_restore_keys() {
    for (step, cleanup) in [("cache", "Cargo"), ("fuzz", "fuzz")] {
        let cache = ACTION
            .split_once(&format!("    - id: {step}\n"))
            .unwrap()
            .1
            .split_once(&format!("    - name: Discard {cleanup}"))
            .unwrap()
            .0;
        assert!(!cache.contains("restore-keys:"));
        assert!(cache.contains("${{ github.sha }}"));
    }
    assert!(ACTION.contains("restore-keys: ${{ steps.policy.outputs.sources-prefix }}"));
    let normalize = ACTION
        .split_once("    - name: Reapply commit mtimes")
        .unwrap()
        .1;
    assert!(normalize.contains(
        "steps.cache.outputs.cache-hit == 'true' || steps.fuzz.outputs.cache-hit == 'true'"
    ));
}

#[test]
fn exact_git_checkout_keeps_workspace_artifacts_fresh() {
    let scratch = xtask::evidence::non_tmp_tempdir().unwrap();
    let root = scratch.path();
    workspace(root);
    let git = |args: &[&str]| {
        success(
            Command::new("git")
                .args(args)
                .current_dir(root)
                .env("GIT_AUTHOR_DATE", "2023-11-14T22:13:20Z")
                .env("GIT_COMMITTER_DATE", "2023-11-14T22:13:20Z")
                .output()
                .unwrap(),
        )
    };
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@invalid",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-qm",
        "pinned source",
    ]);
    let revision = git(&["rev-parse", "HEAD"]);
    let archive_time = SystemTime::UNIX_EPOCH
        + Duration::from_secs(
            git(&["show", "-s", "--format=%ct", "HEAD"])
                .trim()
                .parse()
                .unwrap(),
        );
    let restore_checkout = step_script("    - name: Reapply commit mtimes");
    let normalize = || {
        success(
            Command::new("bash")
                .args(["-c", &restore_checkout])
                .env("GITHUB_SHA", revision.trim())
                .env("GITHUB_WORKSPACE", root)
                .current_dir(root)
                .output()
                .unwrap(),
        );
    };
    let source = root.join("payload/src/lib.rs");
    assert!(fs::metadata(&source).unwrap().modified().unwrap() > archive_time);
    assert!(!fresh(&build(root, false), BINARY));
    File::open(&source)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(60))
        .unwrap();
    normalize();
    assert_eq!(
        fs::metadata(&source).unwrap().modified().unwrap(),
        archive_time
    );
    let warm = build(root, false);
    assert!(fresh(&warm, "payload"));
    assert!(fresh(&warm, BINARY));
    assert!(fresh(&warm, "itoa"));
    assert_eq!(binary_output(root, false), "A 42\n");
    fs::write(&source, "pub const TAG: &str = \"B\";\n").unwrap();
    let dirty = Command::new("bash")
        .args(["-c", &restore_checkout])
        .env("GITHUB_SHA", revision.trim())
        .env("GITHUB_WORKSPACE", root)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(!dirty.status.success());
    assert!(fs::read_to_string(&source).unwrap().contains("\"B\""));

    let index = root.join(".git/index");
    fs::write(&index, b"invalid index").unwrap();
    let unreadable_index = Command::new("bash")
        .args(["-c", &restore_checkout])
        .env("GITHUB_SHA", revision.trim())
        .env("GITHUB_WORKSPACE", root)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(!unreadable_index.status.success());
    assert!(fs::read_to_string(&source).unwrap().contains("\"B\""));
}
