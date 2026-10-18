use std::path::PathBuf;
use std::process::Command;

#[test]
fn r3_browser_supervisor_passes_the_pinned_native_entry_helper() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let script = r#"
import ast, copy, pathlib, sys, types
root = pathlib.Path(sys.argv[1])
supervisor_path = root / 'tools/ui/browser_supervisor.py'
browser_path = root / 'tools/ui/live_browser_check.py'
supervisor_tree = ast.parse(supervisor_path.read_text())
browser_tree = ast.parse(browser_path.read_text())
supervisor_class = next(node for node in supervisor_tree.body
                        if isinstance(node, ast.ClassDef)
                        and node.name == 'BrowserSupervisor')
supervisor_main = next(node for node in supervisor_class.body
                       if isinstance(node, ast.FunctionDef) and node.name == 'main')
launch_call = next(node for node in ast.walk(supervisor_main)
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
