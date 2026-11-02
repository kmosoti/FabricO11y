use super::{capture, capture_to, output, verify};
use std::{fs, path::Path};
use tempfile::TempDir;

fn jj(root: &Path, args: &[&str]) -> String {
    output("jj", args, root).unwrap()
}

fn repository() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    jj(dir.path(), &["git", "init", "--colocate"]);
    jj(
        dir.path(),
        &["config", "set", "--repo", "user.name", "Fixture"],
    );
    jj(
        dir.path(),
        &[
            "config",
            "set",
            "--repo",
            "user.email",
            "fixture@example.invalid",
        ],
    );
    dir
}

#[test]
#[ignore = "extensive: requires Jujutsu"]
fn workflow_archive_exclusions_and_rewrites() {
    let dir = repository();
    let root = dir.path();
    fs::write(root.join(".git/info/exclude"), "/backlog/*\n").unwrap();
    fs::create_dir(root.join("backlog")).unwrap();
    fs::write(root.join("backlog/private"), "local").unwrap();
    fs::write(root.join("old"), "archive").unwrap();
    jj(root, &["util", "snapshot"]);
    let old = jj(root, &["log", "--no-graph", "-r", "@", "-T", "commit_id"]);
    jj(root, &["bookmark", "create", "archive/test", "-r", &old]);
    jj(root, &["new", &old]);
    fs::remove_file(root.join("old")).unwrap();
    fs::write(root.join("fresh"), "new").unwrap();
    jj(root, &["util", "snapshot"]);
    let new = jj(root, &["log", "--no-graph", "-r", "@", "-T", "commit_id"]);
    assert_ne!(old, new);
    assert_eq!(
        jj(root, &["file", "show", "-r", "archive/test", "old"]),
        "archive"
    );
    assert_eq!(jj(root, &["file", "list"]), "fresh");
    fs::write(root.join("fresh"), "changed").unwrap();
    jj(root, &["util", "snapshot"]);
    assert_ne!(
        jj(root, &["log", "--no-graph", "-r", "@", "-T", "commit_id"]),
        new
    );
    assert_eq!(
        fs::read_to_string(root.join("backlog/private")).unwrap(),
        "local"
    );
}

#[test]
#[ignore = "extensive: requires Jujutsu and Rust"]
fn workflow_candidate_refusals_preserve_evidence() {
    let dir = repository();
    let root = dir.path();
    fs::write(root.join("source"), "candidate").unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    let file = artifacts.path().join("candidate.txt");
    capture_to(root, &file).unwrap();
    let original = fs::read_to_string(&file).unwrap();
    fs::write(&file, format!("{original}wrong toolchain\n")).unwrap();
    assert!(
        verify(root, &file)
            .unwrap_err()
            .contains("stale candidate or toolchain")
    );
    fs::write(&file, &original).unwrap();
    let work = artifacts
        .path()
        .join(format!("verify-{}", original.lines().next().unwrap()));
    fs::create_dir(&work).unwrap();
    fs::write(work.join("evidence"), "retain").unwrap();
    assert!(verify(root, &file).unwrap_err().contains("already exists"));
    assert_eq!(fs::read_to_string(work.join("evidence")).unwrap(), "retain");
    let forbidden = root.join("forbidden-candidate.txt");
    assert!(capture_to(root, &forbidden).is_err());
    assert!(!forbidden.exists());
    assert!(capture_to(root, Path::new("relative.txt")).is_err());
    fs::write(root.join("source"), "modified").unwrap();
    assert!(
        verify(root, &file)
            .unwrap_err()
            .contains("stale candidate or toolchain")
    );
}

#[test]
#[ignore = "extensive: requires Git, Jujutsu and Rust"]
fn workflow_git_checkout_requires_jj_initialization() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    output("git", &["init", "--initial-branch=main"], root).unwrap();
    fs::write(root.join("source"), "candidate").unwrap();
    assert!(capture(root).unwrap_err().contains("There is no jj repo"));
    jj(root, &["git", "init", "--colocate"]);
    assert!(xtask::valid_commit(
        capture(root).unwrap().lines().next().unwrap()
    ));
}
