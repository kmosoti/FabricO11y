use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn direct_release_callers_forward_the_explicit_native_entry_pin() {
    let root = root();
    let script = r#"
import ast, pathlib, sys, types
root = pathlib.Path(sys.argv[1])
cases = [
    ('tools/qualification/release_main.py', 'run'),
    ('tools/qualification/release_query.py', 'main'),
    ('tools/qualification/release_stress.py', 'main'),
]
expected_path = pathlib.Path('/owned/xtask-browser-enter-exec')
expected_sha = 'a' * 64

def candidate(*paths, mode='segment', server_cpus='0-1', supervisor_cpus='2-3',
              browser_entry_helper, browser_entry_helper_sha256):
    assert browser_entry_helper == expected_path
    assert browser_entry_helper_sha256 == expected_sha
    return ('fixture', paths, mode, server_cpus, supervisor_cpus,
            browser_entry_helper, browser_entry_helper_sha256)

args = types.SimpleNamespace(
    deb_receipt='deb', rpm_receipt='rpm', out='out', mode='segment',
    server_cpus='0-1', worker_cpus='2-3',
    browser_entry_helper=expected_path,
    browser_entry_helper_sha256=expected_sha)
for relative, function_name in cases:
    path = root / relative
    tree = ast.parse(path.read_text())
    function = next(node for node in tree.body
                    if isinstance(node, ast.FunctionDef) and node.name == function_name)
    calls = [node for node in ast.walk(function)
             if isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
             and node.func.id == 'CandidateFixture']
    assert len(calls) == 1, (relative, len(calls))
    expression = ast.Expression(body=calls[0])
    ast.fix_missing_locations(expression)
    result = eval(compile(expression, str(path), 'eval'),
                  {'args': args, 'CandidateFixture': candidate})
    assert result[0] == 'fixture', relative

# The actual call shape must fail closed when either parsed input is absent.
path = root / cases[0][0]
tree = ast.parse(path.read_text())
function = next(node for node in tree.body
                if isinstance(node, ast.FunctionDef) and node.name == cases[0][1])
call = next(node for node in ast.walk(function)
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
            and node.func.id == 'CandidateFixture')
expression = ast.Expression(body=call)
ast.fix_missing_locations(expression)
for field in ('browser_entry_helper', 'browser_entry_helper_sha256'):
    missing = vars(args).copy()
    del missing[field]
    try:
        eval(compile(expression, str(path), 'eval'),
             {'args': types.SimpleNamespace(**missing), 'CandidateFixture': candidate})
    except AttributeError:
        pass
    else:
        raise AssertionError('direct release call accepted absent helper input: ' + field)
"#;
    let output = Command::new("python3")
        .args(["-B", "-c", script])
        .arg(root)
        .output()
        .expect("Python 3 is needed to execute the current release caller shapes");
    assert!(
        output.status.success(),
        "direct release helper handoff failed (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn non_soak_production_access_requires_and_forwards_the_explicit_pin() {
    let root = root();
    let source = root.join("tools/qualification/production_access.py");
    let script = r#"
import hashlib, importlib.util, os, pathlib, subprocess, sys, tempfile, types
source = pathlib.Path(sys.argv[1])

def digest(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()

with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as name:
    scratch = pathlib.Path(name)
    helpers = scratch / 'helpers'
    helpers.mkdir()
    for filename in ('spindle_sim', 'server_dump', 'spool_dump'):
        path = helpers / filename
        path.write_bytes(b'helper')
        path.chmod(0o700)
    work = scratch / 'work'; work.mkdir()
    bins = work / 'bins'; bins.mkdir()
    storage = scratch / 'storage'; storage.mkdir()
    observed = []
    runtime = types.ModuleType('release_runtime')
    runtime.STORAGE = storage
    runtime.digest = digest
    runtime.directory_bytes = lambda path: 0
    runtime.cpu_pair = lambda value: {0, 1} if value == '0-1' else {4, 5}
    class CandidateFixture:
        def __init__(self, *paths, server_cpus, supervisor_cpus,
                     browser_entry_helper, browser_entry_helper_sha256):
            observed.append((paths, server_cpus, supervisor_cpus,
                             browser_entry_helper, browser_entry_helper_sha256))
            self.work, self.bins, self.helpers = work, bins, helpers
            self.input_hashes, self.receipt = {}, {}
    runtime.CandidateFixture = CandidateFixture
    runtime.WorkloadReader = type('WorkloadReader', (), {})
    sys.modules['release_runtime'] = runtime
    spec = importlib.util.spec_from_file_location('production_access_under_test', source)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.subprocess.check_output = lambda *args, **kwargs: b'0 storage\n'
    helper = scratch / 'xtask-browser-enter-exec'
    helper.write_bytes(b'pinned artifact')
    helper.chmod(0o700)
    pin = digest(helper)
    args = types.SimpleNamespace(production_access=True,
        deb_receipt=scratch / 'deb', rpm_receipt=scratch / 'rpm',
        production_out=scratch / 'out', production_server_cpus='0-1',
        browser_entry_helper=helper, browser_entry_helper_sha256=pin)
    module.ProductionAccess(args, scratch / 'raw', soak=False)
    assert len(observed) == 1, observed
    assert observed[0][3:] == (helper, pin), observed

    # Missing or altered identity refuses before CandidateFixture's effect seam.
    for changed in (None, '0' * 64):
        observed.clear()
        args.browser_entry_helper_sha256 = changed
        try:
            module.ProductionAccess(args, scratch / 'raw', soak=False)
        except (ValueError, RuntimeError):
            pass
        else:
            raise AssertionError('invalid native helper pin was accepted')
        assert not observed, 'invalid helper identity reached CandidateFixture'
"#;
    let output = Command::new("python3")
        .args(["-B", "-c", script])
        .arg(source)
        .output()
        .expect("Python 3 is needed to execute ProductionAccess with bounded stubs");
    assert!(
        output.status.success(),
        "non-soak production helper handoff failed (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
