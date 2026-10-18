use std::path::PathBuf;
use std::process::Command;

#[test]
fn r3_browser_supervisor_passes_the_pinned_native_entry_helper() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let script = r#"
import ast, copy, pathlib, re, sys, types
root = pathlib.Path(sys.argv[1])
supervisor_path = root / 'tools/ui/browser_supervisor.py'
browser_path = root / 'tools/ui/live_browser_check.py'
soak_path = root / 'tools/qualification/soak_r3.py'
production_path = root / 'tools/qualification/production_access.py'
runtime_path = root / 'tools/qualification/release_runtime.py'
bridge_path = root / 'tools/qualification/console_bridge.py'
native_fixture_path = root / 'tools/ui/browser_supervisor_native.py'
supervisor_tree = ast.parse(supervisor_path.read_text())
browser_tree = ast.parse(browser_path.read_text())
soak_tree = ast.parse(soak_path.read_text())
production_tree = ast.parse(production_path.read_text())
runtime_tree = ast.parse(runtime_path.read_text())
bridge_tree = ast.parse(bridge_path.read_text())
native_fixture_tree = ast.parse(native_fixture_path.read_text())
supervise = next(node for node in supervisor_tree.body
                 if isinstance(node, ast.FunctionDef) and node.name == 'supervise')
launch_call = next(node for node in ast.walk(supervise)
                   if isinstance(node, ast.Call)
                   and isinstance(node.func, ast.Attribute)
                   and isinstance(node.func.value, ast.Name)
                   and node.func.value.id == 'browser'
                   and node.func.attr == 'browser_group_launch')
browser_launch = next(node for node in browser_tree.body
                      if isinstance(node, ast.FunctionDef)
                      and node.name == 'browser_group_launch')
required = [arg.arg for arg, default in zip(
    browser_launch.args.kwonlyargs, browser_launch.args.kw_defaults)
    if default is None]
assert required == ['entry_helper', 'entry_helper_sha256'], required

def method(tree, class_name, method_name):
    owner = next(node for node in tree.body
                 if isinstance(node, ast.ClassDef) and node.name == class_name)
    return next(node for node in owner.body
                if isinstance(node, ast.FunctionDef) and node.name == method_name)

def calls(node, name):
    return [item for item in ast.walk(node) if isinstance(item, ast.Call)
            and ((isinstance(item.func, ast.Name) and item.func.id == name)
                 or (isinstance(item.func, ast.Attribute) and item.func.attr == name))]

def keyword_names(call):
    return {item.arg for item in call.keywords if item.arg is not None}

# Admission and each existing composition edge must carry the same explicit
# path/hash pair; no ambient lookup or alternative launch route is accepted.
admit = next(node for node in soak_tree.body
             if isinstance(node, ast.FunctionDef) and node.name == 'admission')
assert any(isinstance(item, ast.Call) and isinstance(item.func, ast.Name)
           and item.func.id == 'pinned_browser_entry_helper' for item in ast.walk(admit))
pin_helper = next(node for node in soak_tree.body
                  if isinstance(node, ast.FunctionDef) and node.name == 'pinned_browser_entry_helper')
require_fn = next(node for node in soak_tree.body
                  if isinstance(node, ast.FunctionDef) and node.name == 'require')
module = ast.Module(body=[require_fn, pin_helper], type_ignores=[])
ast.fix_missing_locations(module)
pin_namespace = {'Path': pathlib.Path, 're': re}
exec(compile(module, str(soak_path), 'exec'), pin_namespace)
valid_pin = {'inputs': {'browser_entry_helper': {
    'path': '/owned/xtask-browser-enter', 'sha256': 'a' * 64}}}
assert pin_namespace['pinned_browser_entry_helper'](valid_pin) == (
    pathlib.Path('/owned/xtask-browser-enter'), 'a' * 64)
for invalid in ({'inputs': {}}, {'inputs': {'browser_entry_helper': {
        'path': 'relative/helper', 'sha256': 'a' * 64}}},
        {'inputs': {'browser_entry_helper': {
        'path': '/owned/helper', 'sha256': 'A' * 64}}}):
    try:
        pin_namespace['pinned_browser_entry_helper'](invalid)
    except ValueError:
        pass
    else:
        raise AssertionError('missing or malformed native entry helper pin was accepted')

measure = next(node for node in soak_tree.body
               if isinstance(node, ast.FunctionDef) and node.name == 'measure')
measure_args = next(call for call in calls(measure, 'SimpleNamespace'))
assert {'browser_entry_helper', 'browser_entry_helper_sha256'} <= keyword_names(measure_args)
production_init = method(production_tree, 'ProductionAccess', '__init__')
assert any(isinstance(node, ast.FunctionDef)
           and node.name == 'browser_entry_options' for node in production_tree.body)
assert any(isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
           and node.func.id == 'browser_entry_options' for node in ast.walk(production_init))
base_calls = [call for call in calls(production_init, '__init__')
              if isinstance(call.func, ast.Attribute)
              and isinstance(call.func.value, ast.Call)
              and isinstance(call.func.value.func, ast.Name)
              and call.func.value.func.id == 'super']
assert base_calls and any(item.arg is None and isinstance(item.value, ast.Name)
                          and item.value.id == 'browser_options'
                          for item in base_calls[0].keywords)
candidate_init = method(runtime_tree, 'CandidateFixture', '__init__')
assert {'browser_entry_helper', 'browser_entry_helper_sha256'} <= {
    arg.arg for arg in candidate_init.args.kwonlyargs}
snapshot_calls = calls(candidate_init, 'snapshot_browser_entry_helper')
assert len(snapshot_calls) == 1, 'candidate must take one owned helper snapshot'
assert any(isinstance(node, ast.Attribute) and node.attr == 'browser_entry_helper_dir'
           for node in ast.walk(candidate_init)), 'helper snapshot must join immutable inputs'
console_calls = calls(candidate_init, 'ConsoleBridge')
option_dicts = [item for item in ast.walk(candidate_init) if isinstance(item, ast.Dict)
                and {'browser_entry_helper', 'browser_entry_helper_sha256'} <= {
                    key.value for key in item.keys if isinstance(key, ast.Constant)}]
assert console_calls and option_dicts
helper_options = next(item for item in option_dicts
                      if any(isinstance(key, ast.Constant)
                             and key.value == 'browser_entry_helper'
                             for key in item.keys))
helper_value = next(value for key, value in zip(helper_options.keys, helper_options.values)
                    if isinstance(key, ast.Constant) and key.value == 'browser_entry_helper')
assert isinstance(helper_value, ast.Attribute) and helper_value.attr == 'browser_entry_helper', helper_value
assert any(isinstance(node, ast.Constant) and node.value == 'caller_sha256'
           for node in ast.walk(candidate_init)), 'receipt must bind caller helper identity'
assert any(isinstance(node, ast.Constant) and node.value == 'snapshot_sha256'
           for node in ast.walk(candidate_init)), 'receipt must bind snapshot helper identity'
assert any(any(item.arg is None and isinstance(item.value, ast.Name)
               and item.value.id == 'browser_options' for item in call.keywords)
           for call in console_calls)
bridge_init = method(bridge_tree, 'ConsoleBridge', '__init__')
supervisor_calls = calls(bridge_init, 'BrowserSupervisor')
assert supervisor_calls and {'browser_entry_helper', 'browser_entry_helper_sha256'} <= keyword_names(supervisor_calls[0])
browser_supervisor_init = method(supervisor_tree, 'BrowserSupervisor', '__init__')
assert {'browser_entry_helper', 'browser_entry_helper_sha256'} <= {
    arg.arg for arg in browser_supervisor_init.args.kwonlyargs}
entry_flags = {item.value for item in ast.walk(browser_supervisor_init)
               if isinstance(item, ast.Constant) and isinstance(item.value, str)}
assert {'--browser-entry-helper', '--browser-entry-helper-sha256'} <= entry_flags
assert {'entry_helper', 'entry_helper_sha256'} <= keyword_names(launch_call)
native_constructor_calls = [call for call in calls(native_fixture_tree, '__init__')
    if isinstance(call.func, ast.Attribute) and isinstance(call.func.value, ast.Name)
    and call.func.value.id == 'instance']
assert len(native_constructor_calls) == 2
assert all({'browser_entry_helper', 'browser_entry_helper_sha256'} <= keyword_names(call)
           for call in native_constructor_calls)
native_main = next(node for node in native_fixture_tree.body
                   if isinstance(node, ast.FunctionDef) and node.name == 'main')
require_binding_line = next(node.lineno for node in ast.walk(native_main)
    if isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id == 'require'
        for target in node.targets) and isinstance(node.value, ast.Attribute)
    and isinstance(node.value.value, ast.Name) and node.value.value.id == 'supervisor'
    and node.value.attr == 'require')
first_require_call_line = min(call.lineno for call in calls(native_main, 'require'))
assert require_binding_line < first_require_call_line, 'fixture pin refusal calls require before binding it'
events = []
class Parser:
    def add_argument(self, *args, **kwargs): pass
    def parse_args(self):
        return types.SimpleNamespace(out=pathlib.Path('/unused/out'),
            browser_entry_helper=pathlib.Path('relative/helper'),
            browser_entry_helper_sha256='a' * 64)
class Argparse:
    ArgumentParser = lambda *args, **kwargs: Parser()
def fixture_require(value, reason):
    events.append(('refuse', reason))
    if not value:
        raise RuntimeError('browser-supervisor-' + reason)
def no_subprocess(*args, **kwargs):
    raise AssertionError('invalid helper reached subprocess/cgroup setup')
fixture_module = ast.Module(body=[copy.deepcopy(native_main)], type_ignores=[])
ast.fix_missing_locations(fixture_module)
fixture_namespace = {'argparse': Argparse, 'Path': pathlib.Path, '__doc__': 'native fixture',
    'supervisor': types.SimpleNamespace(browser=types.SimpleNamespace(
        require_limits=lambda: events.append(('limits', None)) or types.SimpleNamespace(name='unused')),
        require=fixture_require),
    'subprocess': types.SimpleNamespace(run=no_subprocess)}
exec(compile(fixture_module, str(native_fixture_path), 'exec'), fixture_namespace)
try:
    fixture_namespace['main']()
except RuntimeError as error:
    assert str(error) == 'browser-supervisor-fixture-entry-helper-pin', error
else:
    raise AssertionError('invalid helper pin did not refuse at fixture entry')
assert events == [('limits', None), ('refuse', 'fixture-entry-helper-pin')], events

# Keep the actual required keyword-only signature while replacing only the
# launch body so this source control has no cgroup/process side effects.
stub = copy.deepcopy(browser_launch)
stub.name = '_signature_preserving_launch'
stub.decorator_list = []
stub.body = [ast.Return(value=ast.Call(func=ast.Name(id='locals', ctx=ast.Load()),
                                       args=[], keywords=[]))]
invoke = ast.FunctionDef(
    name='_invoke_actual_supervisor_launch',
    args=ast.arguments(posonlyargs=[],
        args=[ast.arg(arg=name) for name in ('browser', 'parent', 'bindings', 'args', 'log', 'os')],
        vararg=None, kwonlyargs=[], kw_defaults=[], kwarg=None, defaults=[]),
    body=[ast.Return(value=copy.deepcopy(launch_call))], decorator_list=[])
module = ast.Module(body=[stub, invoke], type_ignores=[])
ast.fix_missing_locations(module)
namespace = {}
exec(compile(module, str(supervisor_path), 'exec'), namespace)
helper = pathlib.Path('/pinned/xtask')
helper_sha = 'a' * 64
browser = types.SimpleNamespace(browser_group_launch=namespace['_signature_preserving_launch'])
args = types.SimpleNamespace(command=['/owned/geckodriver'],
    browser_entry_helper=helper, browser_entry_helper_sha256=helper_sha)
result = namespace['_invoke_actual_supervisor_launch'](
    browser, object(), [], args, object(), types.SimpleNamespace(environ={}))
assert result['entry_helper'] == helper, result
assert result['entry_helper_sha256'] == helper_sha, result
try:
    namespace['_invoke_actual_supervisor_launch'](
        browser, object(), [], types.SimpleNamespace(command=['/owned/geckodriver']),
        object(), types.SimpleNamespace(environ={}))
except AttributeError as error:
    assert 'browser_entry_helper' in str(error), error
else:
    raise AssertionError('supervisor caller accepted absent helper identity')

# Representative append-after-release style omission: removing both exact
# helper keywords from a copied call must remain rejected by the real signature.
mutant_call = copy.deepcopy(launch_call)
mutant_call.keywords = [keyword for keyword in mutant_call.keywords
                        if keyword.arg not in ('entry_helper', 'entry_helper_sha256')]
mutant_invoke = copy.deepcopy(invoke)
mutant_invoke.name = '_invoke_omitted_helper_mutant'
mutant_invoke.body = [ast.Return(value=mutant_call)]
mutant_module = ast.Module(body=[stub, mutant_invoke], type_ignores=[])
ast.fix_missing_locations(mutant_module)
mutant_namespace = {}
exec(compile(mutant_module, str(supervisor_path), 'exec'), mutant_namespace)
try:
    mutant_namespace['_invoke_omitted_helper_mutant'](
        browser, object(), [], args, object(), types.SimpleNamespace(environ={}))
except TypeError as error:
    message = str(error)
    assert 'entry_helper' in message and 'entry_helper_sha256' in message, message
else:
    raise AssertionError('omitted-helper mutant was accepted by the actual launch signature')
"#;
    let output = Command::new("python3")
        .args(["-B", "-c", script])
        .arg(root)
        .output()
        .expect("Python 3 is needed to exercise the R3 supervisor caller AST");
    assert!(
        output.status.success(),
        "R3 BrowserSupervisor caller did not supply the pinned native entry helper (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
