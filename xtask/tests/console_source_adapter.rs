use serde_json::{Map, Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    product: PathBuf,
    build: PathBuf,
    assets: Map<String, Value>,
    receipt_pin: String,
    commit: String,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "failed console-source adapter fixture retained: {}",
                self.root.display()
            );
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn fixture() -> Fixture {
    let scratch =
        PathBuf::from(std::env::var_os("FABRIC_SCRATCH_ROOT").expect("owned DATA scratch"));
    let root = scratch.join(format!(
        "console-source-adapter-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    let product = root.join("product");
    fs::create_dir_all(product.join("src")).unwrap();
    git(&product, &["init", "-q"]);
    git(&product, &["config", "user.name", "Fixture"]);
    git(
        &product,
        &["config", "user.email", "fixture@example.invalid"],
    );
    fs::write(product.join("Cargo.lock"), "product-lock\n").unwrap();
    fs::write(product.join("src/lib.rs"), "pub fn product() -> u8 { 1 }\n").unwrap();
    git(&product, &["add", "--all"]);
    git(&product, &["commit", "-m", "frozen product"]);
    let commit = git(&product, &["rev-parse", "HEAD"]);

    let build = root.join("build");
    fs::create_dir_all(build.join("dist")).unwrap();
    let bodies = [
        (
            "index.html",
            b"<!doctype html><title>fixture</title>".to_vec(),
        ),
        (
            "service-worker.js",
            b"self.addEventListener('fetch',()=>{});".to_vec(),
        ),
        ("manifest.webmanifest", b"{\"name\":\"fixture\"}".to_vec()),
        (
            "icon-192.png",
            b"\x89PNG\r\n\x1a\n\0\0\0\0IEND\0\0\0\0".to_vec(),
        ),
        (
            "icon-512.png",
            b"\x89PNG\r\n\x1a\n\0\0\0\0IEND\0\0\0\0".to_vec(),
        ),
        ("client.wasm", b"\0asm\x01\0\0\0".to_vec()),
    ];
    let mut assets = Map::new();
    for (name, body) in bodies {
        fs::write(build.join("dist").join(name), &body).unwrap();
        assets.insert(
            name.to_owned(),
            json!({"bytes": body.len(), "sha256": xtask::sha256_hex(&body)}),
        );
    }
    let mut sources = Map::new();
    for name in ["Cargo.lock", "src/lib.rs"] {
        sources.insert(
            name.to_owned(),
            json!(xtask::sha256_hex(&fs::read(product.join(name)).unwrap())),
        );
    }
    let receipt = serde_json::to_vec(&json!({
        "exit": 0, "assets": assets, "source_sha256": sources,
        "source": "/receipt/selected/this/is/not/authority",
        "headers": {"Content-Security-Policy": "default-src 'self'"}
    }))
    .unwrap();
    fs::write(build.join("build.json"), &receipt).unwrap();
    let receipt_pin = xtask::sha256_hex(&receipt);
    Fixture {
        root,
        product,
        build,
        assets,
        receipt_pin,
        commit,
    }
}

fn helper() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_xtask"))
}

fn python_stage(
    f: &Fixture,
    output: &Path,
    helper: &Path,
    helper_pin: &str,
    mutation: &str,
) -> Output {
    let stage = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tools/packaging/stage_console.py");
    let script = r#"
import hashlib, importlib.util, json, pathlib, shutil, subprocess, sys
path, build, output, root, commit, pin, helper, helper_pin, mutation = sys.argv[1:]
spec = importlib.util.spec_from_file_location('stage_console', path)
module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
evidence = {}
snapshot = pathlib.Path(output).parent / 'helper-snapshot'
shutil.copyfile(helper, snapshot); snapshot.chmod(0o700)
if mutation == 'mutate-after-mkdir':
    original_mkdir = pathlib.Path.mkdir
    expected_index = (pathlib.Path(build) / 'dist/index.html').read_bytes()
    destination = pathlib.Path(output)
    def mutate_after_mkdir(self, *args, **kwargs):
        result = original_mkdir(self, *args, **kwargs)
        if self == destination:
            (pathlib.Path(build) / 'dist/index.html').write_bytes(b'changed after verification')
        return result
    pathlib.Path.mkdir = mutate_after_mkdir
def run_helper(snapshot, source_root, source_commit, build_root, receipt_pin):
    command = [str(snapshot), 'console-source', '--source-root', str(source_root),
        '--source-commit', source_commit, '--build-root', str(build_root), '--receipt-sha256', receipt_pin]
    (pathlib.Path(output).parent / 'helper-invoked').write_text('started')
    result = subprocess.run(command, stdin=subprocess.DEVNULL, capture_output=True, timeout=90, check=False)
    report = json.loads(result.stdout) if result.returncode == 0 else None
    if result.returncode == 0:
        (pathlib.Path(output).parent / 'helper-completed').write_text('matched')
    if mutation == 'after-helper':
        p = pathlib.Path(build) / 'build.json'
        p.write_bytes(p.read_bytes() + b' ')
    return report, result.returncode, len(result.stdout), len(result.stderr)
if mutation == 'wrong-root': root = str(pathlib.Path(root) / 'missing-product')
if mutation == 'wrong-commit': commit = '0' * 40
if mutation == 'wrong-pin': pin = '0' * 64
if mutation == 'oversized-receipt':
    (pathlib.Path(build) / 'build.json').write_bytes(b' ' * (4 * 1024 * 1024 + 1))
if mutation.startswith('asset-') or mutation in ('png-time', 'declared-budget', 'malformed-size', 'sparse-asset'):
    build_path = pathlib.Path(build); dist = build_path / 'dist'
    body_path = dist / 'index.html'
    receipt_path = build_path / 'build.json'
    document = json.loads(receipt_path.read_text())
    if mutation == 'asset-changed': body_path.write_bytes(body_path.read_bytes() + b' changed')
    elif mutation == 'asset-missing': body_path.unlink()
    elif mutation == 'asset-unlisted': (dist / 'unexpected.bin').write_bytes(b'unlisted')
    elif mutation == 'asset-symlink':
        body_path.unlink(); body_path.symlink_to('service-worker.js')
    elif mutation == 'png-time':
        png = dist / 'icon-192.png'
        png.write_bytes(b'\x89PNG\r\n\x1a\n' + b'\0\0\0\0tIME\0\0\0\0' + b'\0\0\0\0IEND\0\0\0\0')
        changed = png.read_bytes()
        document['assets']['icon-192.png'] = {'bytes': len(changed), 'sha256': hashlib.sha256(changed).hexdigest()}
    elif mutation == 'asset-budget':
        large = b'x' * (8 * 1024 * 1024 + 1)
        (dist / 'large.wasm').write_bytes(large)
        document['assets']['large.wasm'] = {'bytes': len(large), 'sha256': hashlib.sha256(large).hexdigest()}
    elif mutation == 'declared-budget':
        document['assets']['large.wasm'] = {'bytes': 8 * 1024 * 1024 + 1, 'sha256': '0' * 64}
    elif mutation == 'malformed-size':
        document['assets']['index.html']['bytes'] = True
    elif mutation == 'sparse-asset':
        import os
        os.truncate(body_path, 64 * 1024 * 1024)
    if mutation != 'sparse-asset':
        receipt_path.write_text(json.dumps(document))
        pin = hashlib.sha256(receipt_path.read_bytes()).hexdigest()
module.stage(pathlib.Path(build), pathlib.Path(output),
    product_source=(pathlib.Path(root), commit, pin, snapshot, helper_pin),
    helper_runner=run_helper, evidence=evidence)
if mutation == 'mutate-after-mkdir' and (pathlib.Path(output) / 'index.html').read_bytes() != expected_index:
    raise AssertionError('staged asset bytes differ from bytes verified before destination creation')
print(json.dumps(evidence))
"#;
    Command::new("python3")
        .args(["-B", "-c", script])
        .arg(stage)
        .arg(&f.build)
        .arg(output)
        .arg(&f.product)
        .arg(&f.commit)
        .arg(&f.receipt_pin)
        .arg(helper)
        .arg(helper_pin)
        .arg(mutation)
        .env(
            "FABRIC_STORAGE_ROOT",
            std::env::var_os("FABRIC_STORAGE_ROOT").unwrap(),
        )
        .output()
        .unwrap()
}

#[test]
fn production_adapter_joins_pinned_product_and_preserves_default_refusal() {
    let f = fixture();
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tools/packaging/stage_console.py");
    let stale = Command::new("python3")
        .args(["-B"])
        .arg(script)
        .arg(&f.build)
        .arg(f.root.join("default-stage"))
        .output()
        .unwrap();
    assert!(!stale.status.success());
    assert!(
        String::from_utf8_lossy(&stale.stderr)
            .contains("console was built from different source: Cargo.lock")
    );
    assert!(!f.root.join("default-stage").exists());

    let native = helper();
    let native_pin = xtask::sha256_hex(&fs::read(&native).unwrap());
    let output = f.root.join("stage");
    let accepted = python_stage(&f, &output, &native, &native_pin, "none");
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    assert_eq!(fs::read_dir(&output).unwrap().count(), f.assets.len() + 2);
    assert_eq!(
        fs::read(output.join("index.html")).unwrap(),
        b"<!doctype html><title>fixture</title>"
    );
}

#[test]
fn production_adapter_rejects_wrong_helper_pin_forged_report_and_receipt_race() {
    let f = fixture();
    let native = helper();
    let native_pin = xtask::sha256_hex(&fs::read(&native).unwrap());
    let wrong_hash_output = f.root.join("wrong-hash");
    let wrong_hash = python_stage(&f, &wrong_hash_output, &native, &"0".repeat(64), "none");
    assert!(!wrong_hash.status.success());
    assert!(
        String::from_utf8_lossy(&wrong_hash.stderr)
            .contains("console source helper snapshot differs from caller pin"),
        "{}",
        String::from_utf8_lossy(&wrong_hash.stderr)
    );
    assert!(!wrong_hash_output.exists());

    for (name, expected) in [
        ("wrong-root", "console source helper failed"),
        ("wrong-commit", "console source helper failed"),
        ("wrong-pin", "console build receipt differs from caller pin"),
    ] {
        let output = f.root.join(name);
        let rejected = python_stage(&f, &output, &native, &native_pin, name);
        assert!(!rejected.status.success(), "{name}");
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&rejected.stderr)
        );
        assert!(!output.exists());
    }

    let fake = f.root.join("fake-helper");
    fs::write(&fake, "#!/usr/bin/python3\nprint('{\"status\":\"matched\",\"source_root\":\"/forged\",\"source_commit\":\"0000000000000000000000000000000000000000\",\"build_root\":\"/forged\",\"receipt_sha256\":\"0000000000000000000000000000000000000000000000000000000000000000\"}')\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
    let fake_pin = xtask::sha256_hex(&fs::read(&fake).unwrap());
    let forged_output = f.root.join("forged");
    let forged = python_stage(&f, &forged_output, &fake, &fake_pin, "none");
    assert!(!forged.status.success());
    assert!(
        String::from_utf8_lossy(&forged.stderr)
            .contains("report identity does not match caller pins")
    );
    assert!(!forged_output.exists());

    let race_output = f.root.join("receipt-race");
    let race = python_stage(&f, &race_output, &native, &native_pin, "after-helper");
    assert!(!race.status.success());
    assert!(
        String::from_utf8_lossy(&race.stderr).contains("receipt changed after source verification")
    );
    assert!(!race_output.exists());
}

#[test]
fn production_adapter_bounds_explicit_receipt_before_helper_invocation() {
    let f = fixture();
    let native = helper();
    let native_pin = xtask::sha256_hex(&fs::read(&native).unwrap());
    let output = f.root.join("oversized-receipt");
    let rejected = python_stage(&f, &output, &native, &native_pin, "oversized-receipt");
    let diagnostic = String::from_utf8_lossy(&rejected.stderr);
    assert!(!rejected.status.success(), "{diagnostic}");
    assert!(
        diagnostic.contains("console build receipt exceeds 4 MiB bound"),
        "{diagnostic}"
    );
    assert!(
        !f.root.join("helper-invoked").exists(),
        "oversized receipt invoked the helper"
    );
    assert!(!output.exists());
}

#[test]
fn production_adapter_checks_asset_failures_only_after_pinned_source_join() {
    for (mutation, expected) in [
        ("asset-changed", "asset differs from console build receipt"),
        (
            "asset-missing",
            "asset inventory differs from build receipt",
        ),
        (
            "asset-unlisted",
            "asset inventory differs from build receipt",
        ),
        ("asset-symlink", "unsafe asset path"),
        ("png-time", "console icon carries time or text metadata"),
        ("asset-budget", "console exceeds 8 MiB budget"),
        ("declared-budget", "console exceeds 8 MiB budget"),
        (
            "malformed-size",
            "asset byte count must be a nonnegative integer",
        ),
        ("sparse-asset", "asset differs from console build receipt"),
    ] {
        let f = fixture();
        let native = helper();
        let native_pin = xtask::sha256_hex(&fs::read(&native).unwrap());
        let output = f.root.join(mutation);
        let rejected = python_stage(&f, &output, &native, &native_pin, mutation);
        assert!(!rejected.status.success(), "{mutation}");
        let diagnostic = String::from_utf8_lossy(&rejected.stderr);
        assert!(
            f.root.join("helper-completed").is_file(),
            "pinned source join did not complete before the asset check"
        );
        assert!(diagnostic.contains(expected), "{mutation}: {diagnostic}");
        assert!(!output.exists());
    }
}

#[test]
fn browser_adapter_rejects_incomplete_or_unextracted_source_tuple_before_fixture_start() {
    let live = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tools/ui/live_browser_check.py");
    let script = r#"
import argparse, importlib.util, pathlib, sys
sys.path.insert(0, str(pathlib.Path(sys.argv[1]).parent))
spec = importlib.util.spec_from_file_location('live_browser_check', sys.argv[1])
module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
exec(sys.argv[2])
parser = argparse.ArgumentParser()
try:
    module.validate_product_source_options(module_args, parser)
except SystemExit as error:
    raise SystemExit(error.code)
raise SystemExit(0)
"#;
    for (source_root, packaged, expected) in [("/data/product", "", 2), ("", "/data/package", 2)] {
        let root_expr = if source_root.is_empty() {
            "None".to_owned()
        } else {
            format!("pathlib.Path('{}')", source_root)
        };
        let args = format!(
            "module_args = type('Args', (), {{'product_source_root': {}, 'product_source_commit': '{}', 'console_receipt_sha256': '{}', 'console_source_helper': pathlib.Path('{}'), 'console_source_helper_sha256': '{}', 'packaged_console': {}}})()\n",
            root_expr,
            "a".repeat(40),
            "b".repeat(64),
            "/data/helper",
            "c".repeat(64),
            if packaged.is_empty() {
                "None".to_owned()
            } else {
                format!("pathlib.Path('{}')", packaged)
            }
        );
        let output = Command::new("python3")
            .args(["-B", "-c", script, live.to_str().unwrap()])
            .env(
                "FABRIC_STORAGE_ROOT",
                std::env::var_os("FABRIC_STORAGE_ROOT").unwrap(),
            )
            .arg(args)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(expected),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("resource group:"));
    }
    let no_tuple = "module_args = type('Args', (), {'product_source_root': None, 'product_source_commit': None, 'console_receipt_sha256': None, 'console_source_helper': None, 'console_source_helper_sha256': None, 'packaged_console': None})()\n";
    let output = Command::new("python3")
        .args(["-B", "-c", script, live.to_str().unwrap()])
        .env(
            "FABRIC_STORAGE_ROOT",
            std::env::var_os("FABRIC_STORAGE_ROOT").unwrap(),
        )
        .env("PRODUCT_SOURCE_ROOT", "/untrusted/environment/selection")
        .arg(no_tuple)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn production_helper_callback_uses_the_existing_owned_group_and_finite_cell_deadline() {
    let live = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tools/ui/live_browser_check.py");
    let script = r#"
import ast, pathlib, sys
tree = ast.parse(pathlib.Path(sys.argv[1]).read_text())
callbacks = [node for node in ast.walk(tree) if isinstance(node, ast.FunctionDef) and node.name == 'verify_console_source']
assert len(callbacks) == 1
calls = [node for node in ast.walk(callbacks[0]) if isinstance(node, ast.Call)]
names = [node.func.id for node in calls if isinstance(node.func, ast.Name)]
assert 'browser_group_launch' in names and 'stop_browser_groups' in names
waits = [node for node in calls if isinstance(node.func, ast.Attribute) and node.func.attr == 'wait']
assert any(any(keyword.arg == 'timeout' and isinstance(keyword.value, ast.Name) and keyword.value.id == 'helper_timeout'
               for keyword in node.keywords) for node in waits)
source = pathlib.Path(sys.argv[1]).read_text()
assert 'helper_timeout = min(90, max(0, deadline - time.monotonic()))' in source
assert 'binding[\'group\'].get(\'kernel_empty\') is True' in source
assert 'binding[\'group\'].get(\'children_reaped\') is True' in source
assert 'helper_log.read(64 * 1024 + 1)' in source
"#;
    let output = Command::new("python3")
        .args(["-B", "-c", script, live.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn verify_binary_identity_checks_every_recorded_snapshot_role_after_accepting_them() {
    let fixture = fixture();
    let live = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tools/ui/live_browser_check.py");
    let script = r#"
import ast, hashlib, pathlib, sys
source = pathlib.Path(sys.argv[1]).read_text()
tree = ast.parse(source)
function = next(node for node in ast.walk(tree) if isinstance(node, ast.FunctionDef) and node.name == 'verify_binary_identity')
root = pathlib.Path(sys.argv[2]); root.mkdir()
roles = ('server', 'spindle', 'fabricctl', 'console_source_helper')
args = type('Args', (), {'migration': True})()
receipt = {'binaries': {}, 'immutable_binary_snapshots': {}, 'native_snapshot_hashes_after': {}}
for role in roles:
    original = root / (role + '-input'); original.write_bytes((role + '-input-bytes').encode())
    snapshot = root / (role + '-snapshot'); snapshot.write_bytes((role + '-snapshot-bytes').encode())
    setattr(args, role, snapshot)
    receipt['binaries'][str(original)] = hashlib.sha256(original.read_bytes()).hexdigest()
    receipt['immutable_binary_snapshots'][role] = {'sha256': hashlib.sha256(snapshot.read_bytes()).hexdigest()}
namespace = {'Path': pathlib.Path, 'hashlib': hashlib, 'args': args, 'receipt': receipt,
             'check': lambda condition, message: (_ for _ in ()).throw(AssertionError(message)) if not condition else None}
exec(compile(ast.Module(body=[function], type_ignores=[]), 'live_browser_check.py', 'exec'), namespace)
namespace['verify_binary_identity']()
(root / 'console_source_helper-snapshot').write_bytes(b'changed-helper')
try:
    namespace['verify_binary_identity']()
except AssertionError as error:
    assert 'all fixture launches retain immutable candidate binary bytes' in str(error), str(error)
except KeyError as error:
    raise AssertionError('helper snapshot was omitted from final identity map') from error
else:
    raise AssertionError('changed helper snapshot was accepted')
"#;
    let output = Command::new("python3")
        .args(["-B", "-c", script, live.to_str().unwrap()])
        .arg(fixture.root.join("binary-identity-control"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn staging_writes_the_asset_bytes_verified_before_destination_creation() {
    let fixture = fixture();
    let native = helper();
    let native_pin = xtask::sha256_hex(&fs::read(&native).unwrap());
    let output = fixture.root.join("mutated-after-mkdir");
    let staged = python_stage(
        &fixture,
        &output,
        &native,
        &native_pin,
        "mutate-after-mkdir",
    );
    assert!(
        staged.status.success(),
        "{}",
        String::from_utf8_lossy(&staged.stderr)
    );
}
