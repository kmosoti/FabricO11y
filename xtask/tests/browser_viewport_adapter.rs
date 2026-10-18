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
