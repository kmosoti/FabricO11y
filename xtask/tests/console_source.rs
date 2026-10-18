use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    base: PathBuf,
    product: PathBuf,
    harness: PathBuf,
    build: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "failed console-source fixture retained: {}",
                self.base.display()
            );
        } else {
            let _ = fs::remove_dir_all(&self.base);
        }
    }
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git available in fixture environment");
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn commit(root: &Path, message: &str) -> String {
    git(root, &["add", "--all"]);
    git(root, &["commit", "-m", message]);
    git(root, &["rev-parse", "HEAD"])
}

fn init_repo(root: &Path) {
    fs::create_dir_all(root).unwrap();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.name", "Fixture"]);
    git(root, &["config", "user.email", "fixture@example.invalid"]);
}

fn fixture() -> Fixture {
    let scratch = PathBuf::from(std::env::var_os("FABRIC_SCRATCH_ROOT").expect("DATA scratch"));
    let base = scratch.join(format!(
        "console-source-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&base).unwrap();
    let product = base.join("product");
    let harness = base.join("harness");
    let build = base.join("build");
    init_repo(&product);
    fs::write(product.join("Cargo.lock"), "product-lock-v1\n").unwrap();
    fs::create_dir(product.join("src")).unwrap();
    fs::write(product.join("src/lib.rs"), "pub fn product() -> u8 { 1 }\n").unwrap();
    commit(&product, "frozen product");

    init_repo(&harness);
    fs::write(harness.join("Cargo.lock"), "harness-lock-v2\n").unwrap();
    fs::create_dir_all(harness.join("tools/packaging")).unwrap();
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    fs::copy(
        workspace.join("tools/packaging/stage_console.py"),
        harness.join("tools/packaging/stage_console.py"),
    )
    .unwrap();
    fs::create_dir_all(harness.join("tools")).unwrap();
    fs::copy(
        workspace.join("tools/resource_group.py"),
        harness.join("tools/resource_group.py"),
    )
    .unwrap();
    commit(&harness, "newer harness");
    fs::create_dir(&build).unwrap();
    Fixture {
        base,
        product,
        harness,
        build,
    }
}

fn receipt(fixture: &Fixture, sources: serde_json::Value) -> String {
    let body =
        serde_json::to_vec(&json!({"exit": 0, "assets": {}, "source_sha256": sources})).unwrap();
    fs::write(fixture.build.join("build.json"), &body).unwrap();
    xtask::sha256_hex(&body)
}

fn raw_receipt(fixture: &Fixture, body: &[u8]) -> String {
    fs::write(fixture.build.join("build.json"), body).unwrap();
    xtask::sha256_hex(body)
}

fn source_inventory(fixture: &Fixture) -> serde_json::Value {
    let lock = fs::read(fixture.product.join("Cargo.lock")).unwrap();
    let source = fs::read(fixture.product.join("src/lib.rs")).unwrap();
    json!({
        "Cargo.lock": xtask::sha256_hex(&lock),
        "src/lib.rs": xtask::sha256_hex(&source)
    })
}

fn command(fixture: &Fixture, commit: &str, pin: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args([
            "console-source",
            "--source-root",
            fixture.product.to_str().unwrap(),
            "--source-commit",
            commit,
            "--build-root",
            fixture.build.to_str().unwrap(),
            "--receipt-sha256",
            pin,
        ])
        .env(
            "FABRIC_STORAGE_ROOT",
            std::env::var_os("FABRIC_STORAGE_ROOT").unwrap(),
        )
        .output()
        .unwrap()
}

fn expect_rejection(fixture: &Fixture, commit: &str, pin: &str, diagnostic: &str) {
    let rejected = command(fixture, commit, pin);
    assert!(!rejected.status.success());
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        stderr.contains(diagnostic),
        "expected {diagnostic:?}, got {stderr}"
    );
}

fn product_commit(fixture: &Fixture) -> String {
    git(&fixture.product, &["rev-parse", "HEAD"])
}

#[test]
fn pinned_product_is_accepted_while_newer_harness_stager_refuses_its_source_pin() {
    let fixture = fixture();
    let product_commit = git(&fixture.product, &["rev-parse", "HEAD"]);
    let harness_commit = git(&fixture.harness, &["rev-parse", "HEAD"]);
    assert_ne!(product_commit, harness_commit);
    assert_ne!(
        xtask::sha256_hex(&fs::read(fixture.product.join("Cargo.lock")).unwrap()),
        xtask::sha256_hex(&fs::read(fixture.harness.join("Cargo.lock")).unwrap())
    );
    let receipt_pin = receipt(&fixture, source_inventory(&fixture));

    let stager = Command::new("python3")
        .args([
            "-B",
            fixture
                .harness
                .join("tools/packaging/stage_console.py")
                .to_str()
                .unwrap(),
            fixture.build.to_str().unwrap(),
            fixture.base.join("staged").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!stager.status.success());
    let diagnostic = String::from_utf8_lossy(&stager.stderr);
    assert!(
        diagnostic.contains("console was built from different source: Cargo.lock"),
        "{diagnostic}"
    );

    let verified = command(&fixture, &product_commit, &receipt_pin);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(report["source_commit"], product_commit);
    assert_eq!(report["source_root"], fixture.product.to_str().unwrap());
    assert_eq!(report["build_root"], fixture.build.to_str().unwrap());
    assert_eq!(report["source_count"], 2);
    assert_eq!(report["status"], "matched");
}

#[test]
fn committed_source_corruption_is_rejected_by_the_receipt_hash_control() {
    let fixture = fixture();
    let old_commit = git(&fixture.product, &["rev-parse", "HEAD"]);
    let receipt_pin = receipt(&fixture, source_inventory(&fixture));

    // This models the meaningful skipped-hash defect: the changed source is
    // committed and caller-pinned, so the clean-HEAD guard cannot mask a
    // missing comparison against the original receipt digest.
    fs::write(
        fixture.product.join("src/lib.rs"),
        "pub fn product() -> u8 { 2 }\n",
    )
    .unwrap();
    let changed_commit = commit(&fixture.product, "corrupt pinned source");
    assert_ne!(old_commit, changed_commit);
    let rejected = command(&fixture, &changed_commit, &receipt_pin);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("artifact hash mismatch"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
}

#[test]
fn cli_rejects_duplicate_flags_before_touching_inputs() {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args([
            "console-source",
            "--source-root",
            "/bad",
            "--source-root",
            "/bad",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("duplicate argument"));

    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["console-source", "--unknown", "x"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown argument"));

    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["console-source", "--source-root", "/bad"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--source-commit required"));
}

#[test]
fn wrong_commit_and_dirty_checkout_are_rejected_specifically() {
    let wrong = fixture();
    let commit = product_commit(&wrong);
    let pin = receipt(&wrong, source_inventory(&wrong));
    expect_rejection(
        &wrong,
        &"0".repeat(40),
        &pin,
        "source HEAD does not match pinned commit",
    );

    let dirty = fixture();
    let commit = product_commit(&dirty);
    let pin = receipt(&dirty, source_inventory(&dirty));
    fs::write(dirty.product.join("src/lib.rs"), "uncommitted change\n").unwrap();
    expect_rejection(&dirty, &commit, &pin, "source checkout is not clean");
}

#[test]
fn externally_forged_source_digest_and_changed_receipt_are_rejected() {
    let forged = fixture();
    let commit = product_commit(&forged);
    let mut sources = source_inventory(&forged);
    sources["src/lib.rs"] = json!("0".repeat(64));
    let pin = receipt(&forged, sources);
    expect_rejection(&forged, &commit, &pin, "artifact hash mismatch");

    let changed = fixture();
    let commit = product_commit(&changed);
    let pin = receipt(&changed, source_inventory(&changed));
    let mut body = fs::read(changed.build.join("build.json")).unwrap();
    body.push(b' ');
    fs::write(changed.build.join("build.json"), body).unwrap();
    expect_rejection(&changed, &commit, &pin, "artifact hash mismatch");
}

#[test]
fn source_symlink_is_rejected_by_descriptor_relative_artifact_read() {
    let fixture = fixture();
    fs::remove_file(fixture.product.join("src/lib.rs")).unwrap();
    std::os::unix::fs::symlink("../Cargo.lock", fixture.product.join("src/lib.rs")).unwrap();
    let changed_commit = commit(&fixture.product, "tracked source symlink");
    let mut sources = source_inventory(&fixture);
    sources["src/lib.rs"] = json!(xtask::sha256_hex(
        &fs::read(fixture.product.join("src/lib.rs")).unwrap()
    ));
    let pin = receipt(&fixture, sources);
    let rejected = command(&fixture, &changed_commit, &pin);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("Too many levels of symbolic links"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
}

#[test]
fn untracked_missing_unsafe_and_excess_source_inventories_are_rejected() {
    let untracked = fixture();
    fs::write(untracked.product.join(".gitignore"), "ignored-source.txt\n").unwrap();
    let commit = commit(&untracked.product, "ignore generated source");
    fs::write(untracked.product.join("ignored-source.txt"), "ignored\n").unwrap();
    let mut sources = source_inventory(&untracked);
    sources["ignored-source.txt"] = json!(xtask::sha256_hex(
        &fs::read(untracked.product.join("ignored-source.txt")).unwrap()
    ));
    let pin = receipt(&untracked, sources);
    expect_rejection(
        &untracked,
        &commit,
        &pin,
        "source inventory contains an untracked or missing path",
    );

    let missing = fixture();
    let commit = product_commit(&missing);
    let mut sources = source_inventory(&missing);
    sources["missing.rs"] = json!("1".repeat(64));
    let pin = receipt(&missing, sources);
    expect_rejection(
        &missing,
        &commit,
        &pin,
        "source inventory contains an untracked or missing path",
    );

    let unsafe_path = fixture();
    let commit = product_commit(&unsafe_path);
    let pin = receipt(&unsafe_path, json!({"../Cargo.lock": "1".repeat(64)}));
    expect_rejection(
        &unsafe_path,
        &commit,
        &pin,
        "source path must be canonical relative POSIX path",
    );

    let excess = fixture();
    let commit = product_commit(&excess);
    let mut sources = serde_json::Map::new();
    for index in 0..513 {
        sources.insert(format!("not-tracked-{index}.rs"), json!("1".repeat(64)));
    }
    let pin = receipt(&excess, serde_json::Value::Object(sources));
    expect_rejection(
        &excess,
        &commit,
        &pin,
        "source inventory must contain 1..=512 paths",
    );
}

#[test]
fn strict_receipt_rejects_duplicate_keys_and_nonzero_or_boolean_exit() {
    let duplicate = fixture();
    let commit = product_commit(&duplicate);
    let body = format!(
        "{{\"exit\":0,\"source_sha256\":{{\"Cargo.lock\":\"{}\",\"Cargo.lock\":\"{}\"}}}}",
        "1".repeat(64),
        "1".repeat(64)
    );
    let pin = raw_receipt(&duplicate, body.as_bytes());
    expect_rejection(&duplicate, &commit, &pin, "duplicate JSON key");

    for exit in ["1", "true"] {
        let fixture = fixture();
        let commit = product_commit(&fixture);
        let sources = serde_json::to_string(&source_inventory(&fixture)).unwrap();
        let body = format!("{{\"exit\":{exit},\"source_sha256\":{sources}}}");
        let pin = raw_receipt(&fixture, body.as_bytes());
        expect_rejection(
            &fixture,
            &commit,
            &pin,
            "build receipt exit must be integer zero",
        );
    }
}

#[test]
fn assumed_unchanged_source_flag_cannot_hide_a_same_commit_mutation() {
    let fixture = fixture();
    let commit = product_commit(&fixture);
    fs::write(
        fixture.product.join("src/lib.rs"),
        "pub fn product() -> u8 { 9 }\n",
    )
    .unwrap();
    git(
        &fixture.product,
        &["update-index", "--assume-unchanged", "src/lib.rs"],
    );
    let pin = receipt(&fixture, source_inventory(&fixture));
    expect_rejection(
        &fixture,
        &commit,
        &pin,
        "source index has assume-unchanged or skip-worktree flags",
    );
}
