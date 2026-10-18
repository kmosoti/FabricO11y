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

#[test]
fn browser_fetch_ledger_consumer_separates_http_responses_from_witnessed_query_cancellation() {
    let live = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tools/ui/live_browser_check.py");
    let script = r#"
import ast, pathlib, sys
tree = ast.parse(pathlib.Path(sys.argv[1]).read_text())
consumer = next(node for node in ast.walk(tree) if isinstance(node, ast.FunctionDef) and node.name == 'capture_api_metadata')
wrapper = next(node for node in ast.walk(tree) if isinstance(node, ast.Call)
               and isinstance(node.func, ast.Name) and node.func.id == 'js'
               and node.args and isinstance(node.args[0], ast.Constant)
               and isinstance(node.args[0].value, str)
               and 'window.fixtureApiHeaders=[]' in node.args[0].value)
wrapper_js = wrapper.args[0].value
failures=[]
for marker in ("state:'pending'", "entry.state='response'", "entry.state='rejected'",
               "error_stage='fetch'", "error_stage='metadata'", "signal_aborted_at_rejection",
               "signal_aborted_at_dispatch", "signal_abort_observed_ms",
               "addEventListener('abort'", "removeEventListener('abort'"):
    if marker not in wrapper_js: failures.append('production wrapper diagnostic is missing '+marker)

namespace = {'receipt': {}, 'js': None, 'check': None}
exec(compile(ast.Module(body=[consumer], type_ignores=[]), 'live_browser_check.py', 'exec'), namespace)
capture = namespace['capture_api_metadata']
headers = {'cache_control': 'no-store', 'csp': "default-src 'none'", 'coop': 'same-origin', 'corp': 'same-origin'}
def response(**changes):
    row = {'path':'/v1/console/query','method':'POST','status':200,'state':'response',
           'started_ms':10.0,'finished_ms':11.0, **headers}
    row.update(changes); return row
def cancellation(**changes):
    row = {'path':'/v1/console/query','method':'POST','status':None,'state':'rejected',
           'error_stage':'fetch','error_name':'AbortError','error_message':'aborted',
           'signal_aborted_at_dispatch':False,'signal_abort_observed_ms':10.5,
           'signal_aborted_at_rejection':True,'started_ms':10.0,'finished_ms':11.0}
    row.update(changes); return row

def run(name, rows, truncated=False):
    checks=[]; receipt={}
    namespace['receipt']=receipt
    namespace['js']=lambda expression: rows if 'fixtureApiHeaders' in expression and 'Truncated' not in expression else truncated
    namespace['check']=lambda condition, label: checks.append(label) if condition else (_ for _ in ()).throw(AssertionError(label))
    try:
        capture(); ok=True; error=''
    except BaseException as caught:
        ok=False; error=type(caught).__name__+': '+str(caught)
    return ok, error, receipt, checks

ok,error,receipt,_=run('valid response',[response()])
if not ok: failures.append('valid response rejected: '+error)
for key,value in [('cache_control','private'),('csp',''),('coop','unsafe-none'),('corp','cross-origin')]:
    ok,error,_,_=run('wrong '+key,[response(),response(**{key:value})])
    if ok: failures.append('wrong '+key+' accepted')
for key in ('cache_control','csp','coop','corp'):
    row=response(); del row[key]
    ok,error,_,_=run('missing '+key,[response(),row])
    if ok: failures.append('missing '+key+' accepted')

valid_abort=cancellation()
ok,error,receipt,_=run('same-signal cancellation',[response(),valid_abort])
if not ok: failures.append('witnessed query cancellation rejected: '+error)
else:
    if receipt.get('actual_api_headers') != [response(),valid_abort]: failures.append('raw ledger was not retained')
    if receipt.get('actual_api_cancellations') != [valid_abort]: failures.append('cancellation was not separately recorded')

missing_observation=cancellation(); missing_observation.pop('signal_abort_observed_ms')
invalid_rows = [
    ('missing abort observation',missing_observation),
    ('abort observed before dispatch',cancellation(signal_abort_observed_ms=9.0)),
    ('signal not aborted at rejection',cancellation(signal_aborted_at_rejection=False)),
    ('abort observed after completion',cancellation(signal_abort_observed_ms=12.0)),
    ('non-finite abort observation',cancellation(signal_abort_observed_ms=float('nan'))),
    ('non-abort rejection',cancellation(error_name='TypeError')),
    ('non-null response status on rejection',cancellation(status=0)),
    ('non-query path',cancellation(path='/v1/console/status')),
    ('non-POST method',cancellation(method='GET')),
    ('metadata exception',cancellation(error_stage='metadata', **headers)),
    ('pending row',response(state='pending')),
    ('null response status',response(status=None)),
    ('non-integer response status',response(status='200')),
    ('boolean response status',response(status=True)),
]
for name,row in invalid_rows:
    rows=[response(),row]
    ok,error,_,_=run(name,rows)
    if ok: failures.append(name+' accepted')
if run('pre-aborted signal',[response(),cancellation(signal_aborted_at_dispatch=True,signal_abort_observed_ms=10.0)])[0] is False:
    failures.append('pre-aborted same-signal cancellation rejected')
if run('cancellation-only ledger',[cancellation()])[0]: failures.append('cancellation-only ledger accepted')
if run('empty ledger',[])[0]: failures.append('empty ledger accepted')
if run('truncated response ledger',[response()],truncated=True)[0]: failures.append('truncated ledger accepted')
if failures:
    raise AssertionError('\n'.join(failures))
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

fn security_registration_wrapper(live: &Path) -> String {
    let extract = r#"
import ast, pathlib, sys
tree = ast.parse(pathlib.Path(sys.argv[1]).read_text())
matches = [node.args[0].value for node in ast.walk(tree)
           if isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
           and node.func.id == 'js' and node.args
           and isinstance(node.args[0], ast.Constant) and isinstance(node.args[0].value, str)
           and 'window.fetch=async(...args)=>' in node.args[0].value
           and 'window.fixtureCompletedRegistration=null' in node.args[0].value]
assert len(matches) == 1, f'expected one security registration wrapper, got {len(matches)}'
sys.stdout.write(matches[0])
"#;
    let output = Command::new("python3")
        .args(["-B", "-c", extract, live.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn security_registration_capture_source_clones_request_body_and_forwards_original_arguments() {
    let live = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tools/ui/live_browser_check.py");
    let wrapper = security_registration_wrapper(&live);
    assert!(wrapper.contains("window.fetch=async(...args)=>"));
    assert!(wrapper.contains("const inspectionInput=input instanceof Request?input.clone():input"));
    assert!(wrapper.contains("const inspected=new Request(inspectionInput,args[1])"));
    assert!(wrapper.find("const inspectionInput=").unwrap()
        < wrapper.find("const inspected=").unwrap(),
        "clone Request input before constructing the effective inspection request");
    assert!(wrapper.contains("await inspected.clone().json()"));
    assert!(wrapper.contains("return original(...args)"));
    assert!(!wrapper.contains("JSON.parse(args[1].body)"));
}

#[test]
#[ignore = "requires the pinned DATA Bun runtime; the coordinator runs this effect fixture explicitly"]
fn security_registration_capture_effect_preserves_request_and_init_inputs() {
    let live = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tools/ui/live_browser_check.py");
    let wrapper = security_registration_wrapper(&live);
    let fixture = fixture();
    let wrapper_path = fixture.root.join("security-registration-wrapper.js");
    let runner_path = fixture.root.join("security-registration-wrapper-test.mjs");
    fs::write(&wrapper_path, wrapper).unwrap();
    let runner = r#"
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const wrapper = readFileSync(Bun.argv[2], 'utf8');
// Minimized defect from the frozen 73cccefc harness: its Request-only capture
// dereferenced args[1].body and threw before fetch dispatch.
const defectiveWrapper = `const original=window.fetch;window.fixtureCompletedRegistration=null;
window.fetch=(...args)=>{const p=new URL(typeof args[0]==='string'?args[0]:args[0].url,location.href).pathname;
if(p==='/v1/console/auth/register/finish' && !window.fixtureCompletedRegistration)
  window.fixtureCompletedRegistration=JSON.parse(args[1].body);
return original(...args)}`;
const endpoint = 'https://console.invalid/v1/console/auth/register/finish';
const noInit = Symbol('no-init-argument');

async function callWith(source, input, init = noInit) {
  const forwarded = [];
  const bodyUsedAtDispatch = [];
  globalThis.location = { href: 'https://console.invalid/' };
  globalThis.window = {
    fixtureCompletedRegistration: null,
    fetch: (...args) => {
      forwarded.push(args);
      bodyUsedAtDispatch.push(args[0] instanceof Request ? args[0].bodyUsed : null);
      return Promise.resolve(new Response(null, { status: 204 }));
    },
  };
  new Function(source)();
  let error = null;
  try {
    if (init === noInit) await window.fetch(input);
    else await window.fetch(input, init);
  } catch (caught) {
    error = caught;
  }
  return { error, forwarded, captured: window.fixtureCompletedRegistration, bodyUsedAtDispatch };
}

const defectiveRequest = new Request(endpoint, {
  method: 'POST', headers: { 'content-type': 'application/json' }, body: '{"id":"old"}',
});
const rejectedOldRequest = await callWith(defectiveWrapper, defectiveRequest);
assert.equal(rejectedOldRequest.error?.name, 'TypeError');
assert.equal(rejectedOldRequest.forwarded.length, 0, 'the original defect must fail before fetch dispatch');
const defectiveControlPayload = { id: 'url-init-old-control' };
const defectiveControlInit = {
  method: 'POST', headers: { 'content-type': 'application/json' },
  body: JSON.stringify(defectiveControlPayload),
};
const oldControl = await callWith(defectiveWrapper, endpoint, defectiveControlInit);
assert.equal(oldControl.error, null);
assert.equal(oldControl.forwarded.length, 1);
assert.deepEqual(oldControl.captured, defectiveControlPayload);

const requestPayload = { id: 'request-input', nested: { value: 1 } };
const requestBody = JSON.stringify(requestPayload);
const request = new Request(endpoint, {
  method: 'POST', headers: { 'content-type': 'application/json' }, body: requestBody,
});
const requestResult = await callWith(wrapper, request);
assert.equal(requestResult.error, null, String(requestResult.error));
assert.deepEqual(requestResult.captured, requestPayload);
assert.equal(requestResult.forwarded.length, 1);
assert.equal(requestResult.forwarded[0].length, 1);
assert.strictEqual(requestResult.forwarded[0][0], request, 'forward the original Request object');
assert.deepEqual(requestResult.bodyUsedAtDispatch, [false], 'original Request body remains readable at dispatch');
assert.equal(await request.clone().text(), requestBody, 'inspection must not consume the original Request body');

const urlPayload = { id: 'url-plus-init' };
const urlInit = {
  method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(urlPayload),
};
const urlResult = await callWith(wrapper, endpoint, urlInit);
assert.equal(urlResult.error, null, String(urlResult.error));
assert.deepEqual(urlResult.captured, urlPayload);
assert.equal(urlResult.forwarded.length, 1);
assert.equal(urlResult.forwarded[0].length, 2);
assert.strictEqual(urlResult.forwarded[0][0], endpoint);
assert.strictEqual(urlResult.forwarded[0][1], urlInit, 'forward the original init object');

const originalPayload = { id: 'request-original-body' };
const overridePayload = { id: 'request-init-body-override' };
const overriddenRequest = new Request(endpoint, {
  method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(originalPayload),
});
const overrideInit = { body: JSON.stringify(overridePayload) };
const overrideResult = await callWith(wrapper, overriddenRequest, overrideInit);
assert.equal(overrideResult.error, null, String(overrideResult.error));
assert.deepEqual(overrideResult.captured, overridePayload);
assert.equal(overrideResult.forwarded.length, 1);
assert.equal(overrideResult.forwarded[0].length, 2);
assert.strictEqual(overrideResult.forwarded[0][0], overriddenRequest);
assert.strictEqual(overrideResult.forwarded[0][1], overrideInit);
assert.deepEqual(overrideResult.bodyUsedAtDispatch, [false], 'overridden original Request body remains readable at dispatch');
assert.equal(await overriddenRequest.clone().text(), JSON.stringify(originalPayload));

const malformed = new Request(endpoint, {
  method: 'POST', headers: { 'content-type': 'application/json' }, body: 'not-json',
});
const malformedResult = await callWith(wrapper, malformed);
assert.equal(malformedResult.error?.name, 'SyntaxError', 'capture parsing errors must remain visible');
assert.equal(malformedResult.forwarded.length, 0, 'capture errors must not be hidden by dispatch');
"#;
    fs::write(&runner_path, runner).unwrap();
    let bun = PathBuf::from(
        std::env::var_os("FABRIC_STORAGE_ROOT").expect("DATA storage root for pinned Bun"),
    )
    .join("tools/bun-1.4.0/bun");
    assert!(bun.is_file(), "pinned Bun runtime is missing: {}", bun.display());
    let expected_sha256 = std::env::var("FABRIC_BUN_SHA256")
        .expect("admitted FABRIC_BUN_SHA256 pin for the Bun effect test");
    assert!(
        expected_sha256.len() == 64
            && expected_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "FABRIC_BUN_SHA256 must be 64 lowercase hexadecimal characters"
    );
    let bun_bytes = fs::read(&bun).expect("read the admitted DATA Bun executable");
    assert_eq!(
        xtask::sha256_hex(&bun_bytes),
        expected_sha256,
        "DATA Bun executable differs from its independently admitted SHA-256"
    );
    let version = Command::new(&bun).arg("--version").output().unwrap();
    assert!(version.status.success(), "{}", String::from_utf8_lossy(&version.stderr));
    assert_eq!(String::from_utf8_lossy(&version.stdout).trim(), "1.4.0");
    let output = Command::new(&bun)
        .arg(&runner_path)
        .arg(&wrapper_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
