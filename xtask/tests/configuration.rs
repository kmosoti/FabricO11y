use serde_json::Value;
use std::{env, fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

#[test]
fn repository_configuration() {
    for file in [
        "AGENTS.md",
        "README.md",
        "CONTRIBUTING.md",
        "docs/direction.md",
    ] {
        assert!(root().join(file).is_file(), "missing {file}");
    }
    let hooks: Value =
        serde_json::from_str(&fs::read_to_string(root().join(".codex/hooks.json")).unwrap())
            .unwrap();
    let events = hooks["hooks"].as_object().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events["SessionStart"][0]["hooks"][0]["command"],
        "sh .codex/hooks/context.sh"
    );
    let agent: toml::Value =
        toml::from_str(&fs::read_to_string(root().join(".codex/agents/research.toml")).unwrap())
            .unwrap();
    assert_eq!(agent["name"].as_str(), Some("fabric_research"));
    let metadata = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1", "--locked"])
        .current_dir(root())
        .output()
        .unwrap();
    assert!(
        metadata.status.success(),
        "{}",
        String::from_utf8_lossy(&metadata.stderr)
    );
    let metadata: Value = serde_json::from_slice(&metadata.stdout).unwrap();
    for package in metadata["packages"].as_array().unwrap() {
        assert_eq!(
            package["license"], "Apache-2.0",
            "missing package license declaration"
        );
    }
}

fn hook_message(receipt: Option<&str>, identity: Option<&str>) -> String {
    let dir = tempfile::tempdir().unwrap();
    let hooks = dir.path().join(".codex/hooks");
    fs::create_dir_all(&hooks).unwrap();
    let script = hooks.join("context.sh");
    fs::copy(root().join(".codex/hooks/context.sh"), &script).unwrap();
    if let Some(receipt) = receipt {
        fs::create_dir(dir.path().join(".local")).unwrap();
        fs::write(dir.path().join(".local/last-verification"), receipt).unwrap();
    }
    let bin = dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let jj = bin.join("jj");
    let body = match identity {
        Some(id) => {
            format!("#!/bin/sh\n[ \"$1\" = --ignore-working-copy ] || exit 9\nprintf '%s' '{id}'\n")
        }
        None => "#!/bin/sh\nexit 127\n".to_owned(),
    };
    fs::write(&jj, body).unwrap();
    fs::set_permissions(&jj, fs::Permissions::from_mode(0o755)).unwrap();
    let path = env::join_paths(
        std::iter::once(bin).chain(env::split_paths(&env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let result = Command::new("sh")
        .arg(&script)
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result: Value = serde_json::from_slice(&result.stdout).unwrap();
    result["systemMessage"].as_str().unwrap().to_owned()
}

#[test]
fn hook_discloses_missing_stale_failed_and_unavailable_verification() {
    let id = "0123456789012345678901234567890123456789";
    let receipt = format!("candidate={id}\nresult=success\n");
    assert!(hook_message(None, None).contains("No local verification receipt"));
    assert!(hook_message(Some(&receipt), Some(id)).contains("A successful receipt"));
    assert!(hook_message(Some(&receipt), Some("different")).contains("stale"));
    assert!(hook_message(Some(&receipt), None).contains("unavailable"));
    let failed = format!("candidate={id}\nresult=failed: compiler error\n");
    assert!(!hook_message(Some(&failed), Some(id)).contains("A successful receipt"));
}
