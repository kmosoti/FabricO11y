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
fn release_command_parsers_reject_a_partial_helper_pair() {
    let root = root();
    let script = r#"
import contextlib, io, pathlib, sys
root = pathlib.Path(sys.argv[1])
sys.path.insert(0, str(root / 'tools/qualification'))
import release_main, release_query, release_stress
seed = '0xA11FA001'
cases = [
    (release_main, ['--deb-receipt', 'deb', '--rpm-receipt', 'rpm', '--out', 'out',
                    '--tier', '10', '--seed', seed]),
    (release_query, ['--deb-receipt', 'deb', '--rpm-receipt', 'rpm', '--out', 'out',
                     '--seed', seed, '--mode', 'segment']),
    (release_stress, ['--deb-receipt', 'deb', '--rpm-receipt', 'rpm', '--out', 'out',
                      '--seed', seed]),
]
for module, required_args in cases:
    for partial in (['--browser-entry-helper', '/owned/helper'],
                    ['--browser-entry-helper-sha256', 'a' * 64]):
        sys.argv = [module.__file__, *required_args, *partial]
        stderr = io.StringIO()
        try:
            with contextlib.redirect_stderr(stderr):
                module.main()
        except SystemExit as error:
            assert error.code == 2, (module.__file__, error.code)
        else:
            raise AssertionError('partial helper pair reached a release caller')
        missing_name = ('--browser-entry-helper-sha256'
                        if partial[0] == '--browser-entry-helper'
                        else '--browser-entry-helper')
        assert missing_name in stderr.getvalue(), (module.__file__, stderr.getvalue())
"#;
    let output = Command::new("python3")
        .args(["-B", "-c", script])
        .arg(root)
        .output()
        .expect("Python 3 is needed to check release CLI admission");
    assert!(
        output.status.success(),
        "release parser accepted or misreported a partial helper pin (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn remaining_supervised_callers_require_and_forward_the_helper_pin() {
    let root = root();
    let script = r#"
import ast, pathlib, sys, types
root = pathlib.Path(sys.argv[1])
pin_path = pathlib.Path('/owned/xtask-browser-enter-exec')
pin_sha = 'b' * 64

def call_expression(relative, owner_name, owner_kind, function_name, matcher):
    path = root / relative
    tree = ast.parse(path.read_text())
    if owner_kind == 'class_method':
        owner = next(node for node in tree.body
                     if isinstance(node, ast.ClassDef) and node.name == owner_name)
        function = next(node for node in owner.body
                        if isinstance(node, ast.FunctionDef) and node.name == function_name)
    else:
        function = next(node for node in tree.body
                        if isinstance(node, ast.FunctionDef) and node.name == function_name)
    matches = [node for node in ast.walk(function)
               if isinstance(node, ast.Call) and matcher(node)] if callable(matcher) else []
    assert len(matches) == 1, (relative, len(matches))
    return path, matches[0]

def has_call(node, name):
    return ((isinstance(node.func, ast.Name) and node.func.id == name)
            or (isinstance(node.func, ast.Attribute) and node.func.attr == name))

def attr_call(name):
    return lambda node: has_call(node, name)

def launch(parent, bindings, label, command, log, environment, *,
           entry_helper, entry_helper_sha256):
    assert entry_helper == pin_path
    assert entry_helper_sha256 == pin_sha
    return ('launch', entry_helper, entry_helper_sha256)

def bridge(*args, browser_group, browser_parent, browser_deadline,
           browser_entry_helper, browser_entry_helper_sha256, **kwargs):
    assert browser_entry_helper == pin_path
    assert browser_entry_helper_sha256 == pin_sha
    return ('bridge', browser_entry_helper, browser_entry_helper_sha256)

cases = []
smoke_path, smoke_call = call_expression(
    'tools/qualification/console_bridge_smoke.py', 'console_bridge_smoke', 'function',
    'supervised_bridge', attr_call('ConsoleBridge'))
cases.append((smoke_path, smoke_call, {
    'ConsoleBridge': bridge, 'origin': 'https://localhost:1',
    'certificate': pathlib.Path('/owned/cert'), 'work': pathlib.Path('/owned/work'),
    'out': pathlib.Path('/owned/out'), 'budget': pathlib.Path('/owned/browser'),
    'held': {'fd': 1, 'path': pathlib.Path('/owned/browser')}, 'deadline': 30.0,
    'entry_helper': pin_path, 'entry_helper_sha256': pin_sha}))

cleanup_path, cleanup_call = call_expression(
    'tools/ui/browser_cleanup_native.py', 'browser_cleanup_native', 'function',
    'main', attr_call('browser_group_launch'))
cases.append((cleanup_path, cleanup_call, {
    'browser': types.SimpleNamespace(browser_group_launch=launch),
    'parent': object(), 'bindings': [], 'mode': 'normal',
    'sys': types.SimpleNamespace(executable='/usr/bin/python3'),
    'script': pathlib.Path('/owned/child.py'), 'cell': pathlib.Path('/owned/cell'),
    'log': object(), 'os': types.SimpleNamespace(environ={}),
    'args': types.SimpleNamespace(browser_entry_helper=pin_path,
                                  browser_entry_helper_sha256=pin_sha)}))

trust_path, trust_call = call_expression(
    'tools/qualification/console_firefox_trust.py', 'console_firefox_trust', 'function', 'main',
    lambda node: isinstance(node.func, ast.Attribute) and node.func.attr == '__init__'
    and isinstance(node.func.value, ast.Name) and node.func.value.id == 'instance')
cases.append((trust_path, trust_call, {
    'instance': types.SimpleNamespace(__init__=bridge),
    'origin': 'https://guide.fabric.test:8443', 'root': pathlib.Path('/owned'),
    'args': types.SimpleNamespace(case='trusted', out=pathlib.Path('/owned/out'),
        browser_entry_helper=pin_path, browser_entry_helper_sha256=pin_sha),
    'budget': {'path': pathlib.Path('/owned/browser')}, 'end': 30.0,
    'hostname': 'guide.fabric.test', 'sha': lambda path: 'c' * 64}))

owner_path, owner_call = call_expression(
    'tools/qualification/owner_walkthrough.py', 'Walkthrough', 'class_method', 'browser',
    lambda node: isinstance(node.func, ast.Attribute) and node.func.attr == '__init__'
    and isinstance(node.func.value, ast.Name) and node.func.value.id == 'instance')
cases.append((owner_path, owner_call, {
    'instance': types.SimpleNamespace(__init__=bridge),
    'self': types.SimpleNamespace(budget={'path': pathlib.Path('/owned/browser')},
        overall_end=30.0, browser_entry_helper=pin_path,
        browser_entry_helper_sha256=pin_sha),
    'origin': 'https://telemetry.example.com:7443', 'leaf': pathlib.Path('/owned/leaf'),
    'work': pathlib.Path('/owned/work'), 'out': pathlib.Path('/owned/out'),
    'ca': pathlib.Path('/owned/ca')}))

for path, call, namespace in cases:
    expression = ast.Expression(body=call)
    ast.fix_missing_locations(expression)
    value = eval(compile(expression, str(path), 'eval'), namespace)
    assert value[1:] == (pin_path, pin_sha), path
"#;
    let output = Command::new("python3")
        .args(["-B", "-c", script])
        .arg(root)
        .output()
        .expect("Python 3 is needed to execute supervised caller shapes");
    assert!(
        output.status.success(),
        "supervised caller omitted the explicit helper pair (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn remaining_supervised_cli_parsers_reject_partial_helper_pairs() {
    let root = root();
    let script = r#"
import contextlib, io, pathlib, sys
root = pathlib.Path(sys.argv[1])
sys.path[:0] = [str(root / 'tools/qualification'), str(root / 'tools/ui')]
import browser_cleanup_native, console_bridge_smoke, console_firefox_trust, owner_walkthrough
cases = [
    (browser_cleanup_native, ['--out', '/unused']),
    (console_bridge_smoke, ['--build', '/unused', '--bin-dir', '/unused', '--out', '/unused']),
    (console_firefox_trust, ['--case', 'trusted', '--out', '/unused', '--pins', '/unused',
                             '--pins-sha256', 'a' * 64]),
    (owner_walkthrough, ['--admission', '/unused', '--admission-sha256', 'a' * 40,
                         '--outer-admission', '/unused', '--run-id', 'fixture',
                         '--server-family', 'debian']),
]
for module, required_args in cases:
    for partial in (['--browser-entry-helper', '/owned/helper'],
                    ['--browser-entry-helper-sha256', 'a' * 64]):
        sys.argv = [module.__file__, *required_args, *partial]
        stderr = io.StringIO()
        try:
            with contextlib.redirect_stderr(stderr):
                module.main()
        except SystemExit as error:
            assert error.code == 2, (module.__file__, error.code)
        else:
            raise AssertionError('partial helper pair reached a supervised browser caller')
        missing = ('--browser-entry-helper-sha256' if partial[0] == '--browser-entry-helper'
                   else '--browser-entry-helper')
        assert missing in stderr.getvalue(), (module.__file__, stderr.getvalue())
"#;
    let output = Command::new("python3")
        .args(["-B", "-c", script])
        .arg(root)
        .output()
        .expect("Python 3 is needed to check supervised browser CLI admission");
    assert!(
        output.status.success(),
        "supervised browser parser accepted or misreported a partial pin (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn owner_browser_helper_matches_the_independent_admission_pin() {
    let root = root();
    let script = r#"
import ast, pathlib, re, sys
source = pathlib.Path(sys.argv[1])
tree = ast.parse(source.read_text())
function = next(node for node in tree.body
                if isinstance(node, ast.FunctionDef)
                and node.name == 'require_admitted_browser_entry_helper')
main = next(node for node in tree.body
            if isinstance(node, ast.FunctionDef) and node.name == 'main')
calls = [(node.lineno, node.func.id) for node in ast.walk(main)
         if isinstance(node, ast.Call) and isinstance(node.func, ast.Name)]
pin_line = min(line for line, name in calls
               if name == 'require_admitted_browser_entry_helper')
systemctl_line = min(node.lineno for node in ast.walk(main)
    if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
    and node.func.attr == 'run'
    and any(isinstance(child, ast.Constant) and child.value == 'systemctl'
            for child in ast.walk(node)))
assert pin_line < systemctl_line, 'admission pin mismatch must refuse before host-limit effects'

module = ast.Module(body=[function], type_ignores=[])
ast.fix_missing_locations(module)
def require(condition, message):
    if not condition: raise ValueError(message)
namespace = {'Path': pathlib.Path, 're': re, 'require': require}
exec(compile(module, str(source), 'exec'), namespace)
check = namespace['require_admitted_browser_entry_helper']
helper = pathlib.Path('/owned/xtask-browser-enter-exec')
pin = 'a' * 64
plan = {'inputs': {'browser_entry_helper': {'path': str(helper), 'sha256': pin}}}
check(plan, helper, pin)
for record in (
    {'path': '/owned/other-helper', 'sha256': pin},
    {'path': str(helper), 'sha256': 'b' * 64},
):
    try: check({'inputs': {'browser_entry_helper': record}}, helper, pin)
    except ValueError: pass
    else: raise AssertionError('owner helper mismatch was accepted')
try: check({'inputs': {}}, helper, pin)
except ValueError: pass
else: raise AssertionError('owner plan without an admitted helper was accepted')
"#;
    let output = Command::new("python3")
        .args(["-B", "-c", script])
        .arg(root.join("tools/qualification/owner_walkthrough.py"))
        .output()
        .expect("Python 3 is needed to execute the owner admission pin check");
    assert!(
        output.status.success(),
        "owner helper pin did not match/refuse the admission exactly (exit {:?})\nstdout:\n{}\nstderr:\n{}",
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
import argparse, hashlib, importlib.util, os, pathlib, subprocess, sys, tempfile, types
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
                     browser_entry_helper, browser_entry_helper_sha256,
                     browser_deadline=None):
            observed.append((paths, server_cpus, supervisor_cpus,
                             browser_entry_helper, browser_entry_helper_sha256,
                             browser_deadline))
            self.work = work / ('work-' + str(len(observed)))
            self.work.mkdir()
            self.bins = self.work / 'bins'; self.bins.mkdir()
            self.helpers = helpers
            self.input_hashes, self.receipt = {}, {}
    runtime.CandidateFixture = CandidateFixture
    runtime.WorkloadReader = type('WorkloadReader', (), {})
    sys.modules['release_runtime'] = runtime
    spec = importlib.util.spec_from_file_location('production_access_under_test', source)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.subprocess.check_output = lambda *args, **kwargs: b'0 storage\n'
    parser = argparse.ArgumentParser()
    module.options(parser)
    for partial in (['--browser-entry-helper', '/owned/helper'],
                    ['--browser-entry-helper-sha256', 'a' * 64]):
        args = parser.parse_args(['--production-access', '--deb-receipt', 'deb',
            '--rpm-receipt', 'rpm', '--production-out', 'out', *partial])
        try:
            module.validate_options(args)
        except ValueError as error:
            assert 'helper' in str(error), str(error)
        else:
            raise AssertionError('production CLI accepted a partial helper pair')
    helper = scratch / 'xtask-browser-enter-exec'
    helper.write_bytes(b'pinned artifact')
    helper.chmod(0o700)
    pin = digest(helper)
    args = types.SimpleNamespace(production_access=True,
        deb_receipt=scratch / 'deb', rpm_receipt=scratch / 'rpm',
        production_out=scratch / 'out', production_server_cpus='0-1',
        browser_entry_helper=helper, browser_entry_helper_sha256=pin)
    for soak, deadline in ((False, None), (True, 25)):
        args.r3_browser_deadline = deadline
        module.ProductionAccess(args, scratch / 'raw', soak=soak)
        assert len(observed) == (2 if soak else 1), observed
        assert observed[-1][3:] == (helper, pin, deadline), observed

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
