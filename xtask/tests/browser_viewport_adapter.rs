use std::path::PathBuf;
use std::process::Command;

#[test]
fn direct_console_helper_launch_is_included_in_process_exit_accounting() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tools/ui/live_browser_check.py");
    let script = r#"
import ast, pathlib, sys
path = pathlib.Path(sys.argv[1])
tree = ast.parse(path.read_text())
callbacks = [node for node in ast.walk(tree)
             if isinstance(node, ast.FunctionDef) and node.name == 'verify_console_source']
assert len(callbacks) == 1

def accounted(callback):
    nodes = list(ast.walk(callback))
    launches = [node.lineno for node in nodes
                if isinstance(node, ast.Assign)
                and any(isinstance(target, ast.Name) and target.id == 'helper'
                        for target in node.targets)
                and isinstance(node.value, ast.Call)
                and isinstance(node.value.func, ast.Name)
                and node.value.func.id == 'browser_group_launch']
    appends = [node.lineno for node in nodes
               if isinstance(node, ast.Expr) and isinstance(node.value, ast.Call)
               and isinstance(node.value.func, ast.Attribute)
               and isinstance(node.value.func.value, ast.Name)
               and node.value.func.value.id == 'processes'
               and node.value.func.attr == 'append'
               and len(node.value.args) == 1
               and isinstance(node.value.args[0], ast.Tuple)
               and len(node.value.args[0].elts) == 2
               and isinstance(node.value.args[0].elts[0], ast.Constant)
               and node.value.args[0].elts[0].value == 'console-source-verifier'
               and isinstance(node.value.args[0].elts[1], ast.Name)
               and node.value.args[0].elts[1].id == 'helper']
    waits = [node.lineno for node in nodes
             if isinstance(node, ast.Assign) and isinstance(node.value, ast.Call)
             and isinstance(node.value.func, ast.Attribute)
             and isinstance(node.value.func.value, ast.Name)
             and node.value.func.value.id == 'helper' and node.value.func.attr == 'wait']
    return (len(launches) == len(appends) == len(waits) == 1
            and launches[0] < appends[0] < waits[0])

callback = callbacks[0]
assert accounted(callback), 'bounded console source child must be recorded before wait'

# Representative accounting defect: a directly launched owned child omitted
# from `processes` must be rejected, leaving group/process counts incomparable.
without_append = path.read_text().replace(
    "                    processes.append(('console-source-verifier', helper))\n", '', 1)
mutant_tree = ast.parse(without_append)
mutant = next(node for node in ast.walk(mutant_tree)
              if isinstance(node, ast.FunctionDef) and node.name == 'verify_console_source')
assert not accounted(mutant), 'accounting control accepted omitted process append'
"#;
    let output = Command::new("python3")
        .args(["-B", "-c", script])
        .arg(source)
        .output()
        .expect("Python 3 is needed to inspect the legacy harness AST");
    assert!(
        output.status.success(),
        "console helper accounting AST regression failed (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn firefox_bidi_viewport_adapter_checks_ack_geometry_and_cleanup_order() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tools/ui/live_browser_check.py");
    let script = r#"
import ast, copy, json, os, pathlib, subprocess, sys, tempfile, time
path = pathlib.Path(sys.argv[1])
text = path.read_text()
tree = ast.parse(text)
main = next(node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == 'main')
functions = {node.name: node for node in ast.walk(main)
             if isinstance(node, ast.FunctionDef)
             and node.name in ('run_viewport', 'verify_viewport_geometry')}
assert set(functions) == {'run_viewport', 'verify_viewport_geometry'}

def fail_if_false(condition, message):
    if not condition:
        raise AssertionError(message)

with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT']) as scratch_name:
    scratch = pathlib.Path(scratch_name)
    helper = scratch / 'pinned-browser-entry-helper'
    helper.write_bytes(b'fixture helper')
    url = 'ws://127.0.0.1:4444/session/fixture'
    handle = 'fixture-context'

    def runner(acknowledgement, exit_code=0, mode='narrow'):
        encoded = acknowledgement if isinstance(acknowledgement, bytes) else json.dumps(acknowledgement).encode()
        processes, bindings, logs, stopped = [], [], [], []
        label = 'browser-bidi-viewport-' + mode
        class Process:
            returncode = exit_code
            def wait(self, timeout):
                assert timeout <= 5
                assert processes == [(label, self)], 'helper process was not registered before wait'
                return exit_code
        process = Process()
        def launch(parent, groups, launched_label, argv, log, environment, **kwargs):
            assert launched_label == label
            assert argv == [str(helper), 'browser-bidi-viewport', '--ws-url', url,
                            '--context', handle, '--mode', mode]
            assert kwargs == {'entry_helper': helper, 'entry_helper_sha256': 'a' * 64}
            assert environment == {'PATH': '/usr/bin:/bin'}
            log.write(encoded)
            groups.append({'group': {'kernel_empty': True, 'children_reaped': True}})
            return process
        namespace = {
            'args': type('Args', (), {'browser':'firefox','browser_entry_helper':helper,
                                      'browser_entry_helper_sha256':'a' * 64})(),
            'viewport_websocket_url':url, 'logs':logs, 'out':scratch,
            'group_parent':object(), 'owned_groups':bindings,
            'deadline':time.monotonic() + 20, 'processes':processes,
            'browser_group_launch':launch,
            'stop_browser_groups':lambda parent, selected: stopped.extend(selected),
            'subprocess':subprocess, 'time':time, 'json':json,
            'check':fail_if_false,
        }
        module = ast.Module(body=[copy.deepcopy(functions['run_viewport']),
                                  copy.deepcopy(functions['verify_viewport_geometry'])],
                           type_ignores=[])
        exec(compile(module, str(path), 'exec'), namespace)
        return namespace, processes, stopped

    valid = {'command':'browsingContext.setViewport','context':handle,'mode':'narrow',
             'result':{},'received_messages':2,'actual_viewport_verified':False}
    namespace, processes, stopped = runner(valid)
    assert namespace['run_viewport']('narrow', handle) == valid
    assert len(processes) == len(stopped) == 1
    try:
        namespace['verify_viewport_geometry']({'width':500,'height':844,'dpr':1}, {'dpr':1})
    except AssertionError:
        pass
    else:
        raise AssertionError('a valid Rust ACK let actual CSS innerWidth 500 satisfy the 390px predicate')

    restore = dict(valid); restore['mode'] = 'restore'
    namespace, processes, stopped = runner(restore, mode='restore')
    assert namespace['run_viewport']('restore', handle) == restore
    assert len(processes) == len(stopped) == 1

    defects = []
    for key, wrong in [('command','wrong'), ('context','other'), ('mode','restore'),
                       ('result',{'unexpected':True}), ('received_messages',True),
                       ('actual_viewport_verified',True)]:
        defect = dict(valid); defect[key] = wrong; defects.append(defect)
    defect = dict(valid); defect.pop('context'); defects.append(defect)
    defect = dict(valid); defect['unexpected'] = 1; defects.append(defect)
    for defect in defects:
        namespace, _, _ = runner(defect)
        try:
            namespace['run_viewport']('narrow', handle)
        except ValueError as error:
            assert 'acknowledgement' in str(error), str(error)
        else:
            raise AssertionError('malformed BiDi acknowledgement was accepted: ' + repr(defect))
    namespace, _, _ = runner(valid, exit_code=1)
    try:
        namespace['run_viewport']('narrow', handle)
    except ValueError as error:
        assert 'exited unsuccessfully' in str(error)
    else:
        raise AssertionError('failed helper was accepted')
    namespace, _, _ = runner(b'x' * (64 * 1024 + 1))
    try:
        namespace['run_viewport']('narrow', handle)
    except ValueError as error:
        assert '64 KiB' in str(error)
    else:
        raise AssertionError('oversized helper report was accepted')

# Source controls bind the endpoint only to Firefox and the existing pinned
# helper, preserve the acceptance predicate, and restore before DELETE after
# failure evidence has been captured.
assert "firefox_capabilities['webSocketUrl'] = True" in text
assert "chrome_capabilities['webSocketUrl']" not in text
run_node = functions['run_viewport']
calls = [node for node in ast.walk(run_node) if isinstance(node, ast.Call)]
assert any(isinstance(node.func, ast.Name) and node.func.id == 'browser_group_launch' for node in calls)
assert "'--ws-url', viewport_websocket_url, '--context', context_handle" in ast.unparse(run_node)
assert "entry_helper=args.browser_entry_helper" in ast.unparse(run_node)
assert 'helper.wait(timeout=helper_timeout)' in ast.unparse(run_node)
assert 'min(5, max(0, deadline - time.monotonic()))' in ast.unparse(run_node)
assert 'helper_log.read(64 * 1024 + 1)' in ast.unparse(run_node)
assert "wd('/window', method='GET')" in text
assert "window.fixtureNextAction.style.fontSize='18px'" in text
assert "document.documentElement.style.zoom=\"200%\"" in text
assert "window.fixtureNextAction.style.whiteSpace='nowrap'" in text
assert "viewport_restore_required = True" in text
assert "viewport_restore_required = False" in text
failure_png = text.index("(out/'failure.png').write_bytes")
failure_text = text.index("(out/'failure-text.txt').write_text")
finally_restore = text.index("run_viewport('restore', viewport_context_handle)", text.index('    finally:', failure_text))
delete = text.index("call('DELETE', '/session/' + session)", finally_restore)
assert failure_png < failure_text < finally_restore < delete
assert 'cleanup_error = error' in text[finally_restore:delete]
"#;
    let output = Command::new("python3")
        .args(["-B", "-c", script])
        .arg(source)
        .output()
        .expect("Python 3 is needed to exercise the legacy adapter AST");
    assert!(
        output.status.success(),
        "Firefox BiDi adapter AST regression failed (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
