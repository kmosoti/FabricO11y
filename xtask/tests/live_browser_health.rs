use std::path::Path;
use std::process::Command;

const AST_REGRESSION: &str = r#"
import ast
import copy
import sys
import types
from pathlib import Path
import urllib.error

root = Path(sys.argv[1])
source_path = root / 'tools/ui/live_browser_check.py'
tree = ast.parse(source_path.read_text())
main = next(node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == 'main')

def wait_call(label):
    found = [node for node in ast.walk(main)
             if isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
             and node.func.id == 'wait' and len(node.args) >= 2
             and isinstance(node.args[1], ast.Constant) and node.args[1].value == label]
    assert len(found) == 1, (label, len(found))
    return found[0]

labels = {
    'initial': 'production TLS server',
    'recovery': 'production server reopened after owner recovery',
    'shell': 'restarted production TLS server',
}
calls = {key: wait_call(label) for key, label in labels.items()}
health = [node for node in main.body
          if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name == 'healthy']

class Process:
    def __init__(self, code=None):
        self.code = code
        self.poll_calls = 0
    def poll(self):
        self.poll_calls += 1
        return self.code

if len(health) != 1:
    # Evaluate the actual recovery call, so the pre-fix regression is the same
    # free-name failure produced by Python when the outer main path reaches it.
    eval(compile(ast.Expression(calls['recovery']), str(source_path), 'eval'),
         {'wait': lambda predicate, label, seconds=25: predicate(), 'server': Process()})
    raise AssertionError('missing actual main-scoped TLS health predicate')

healthy_node = health[0]
assert [arg.arg for arg in healthy_node.args.args] == ['process'], 'health predicate needs explicit process argument'
assert len(healthy_node.args.defaults) == 0, 'health predicate must not capture a default process'

class Response:
    def __init__(self, status):
        self.status = status
    def __enter__(self):
        return self
    def __exit__(self, *args):
        return False

class Opener:
    def __init__(self):
        self.mode = 200
        self.requests = []
    def urlopen(self, url, **kwargs):
        self.requests.append((url, kwargs))
        if isinstance(self.mode, BaseException):
            raise self.mode
        return Response(self.mode)

opener = Opener()
context = object()
namespace = {
    'origin': 'https://fixture.invalid',
    'context': context,
    'urllib': types.SimpleNamespace(
        request=types.SimpleNamespace(urlopen=opener.urlopen),
        error=types.SimpleNamespace(URLError=urllib.error.URLError)),
}
module = ast.Module(body=[copy.deepcopy(healthy_node)], type_ignores=[])
exec(compile(module, str(source_path), 'exec'), namespace)

def fake_wait(predicate, label, seconds=25):
    assert seconds == 25, (label, seconds)
    assert label in labels.values(), label
    return predicate()

namespace['wait'] = fake_wait

def actual_wait(key, process):
    predicate = calls[key].args[0]
    assert isinstance(predicate, ast.Lambda), (key, ast.dump(predicate))
    body = predicate.body
    assert isinstance(body, ast.Call) and isinstance(body.func, ast.Name) and body.func.id == 'healthy', (key, ast.dump(body))
    assert len(body.args) == 1 and isinstance(body.args[0], ast.Name) and body.args[0].id == 'server', (key, ast.dump(body))
    namespace['server'] = process
    return eval(compile(ast.Expression(calls[key]), str(source_path), 'eval'), namespace)

first = Process()
opener.mode = 200
assert actual_wait('initial', first) is True
assert first.poll_calls == 1
assert len(opener.requests) == 1
url, kwargs = opener.requests[-1]
assert url == 'https://fixture.invalid/v1/health'
assert kwargs == {'context': context, 'timeout': 1}

for mode in (503, urllib.error.URLError('connection refused')):
    opener.mode = mode
    before = len(opener.requests)
    assert actual_wait('initial', Process()) is False
    assert len(opener.requests) == before + 1

opener.mode = 200
dead = Process(code=17)
before = len(opener.requests)
try:
    actual_wait('initial', dead)
except RuntimeError as error:
    assert str(error) == 'server exited; inspect server.log', str(error)
else:
    raise AssertionError('dead server was accepted')
assert dead.poll_calls == 1
assert len(opener.requests) == before, 'dead process must fail before a health request'

previous = first
for key in ('recovery', 'shell'):
    replacement = Process()
    before_poll = previous.poll_calls
    before_requests = len(opener.requests)
    assert actual_wait(key, replacement) is True
    assert replacement.poll_calls == 1, (key, 'replacement process was not probed')
    assert previous.poll_calls == before_poll, (key, 'stale process was probed')
    assert len(opener.requests) == before_requests + 1
    previous = replacement

print('actual health predicate and three actual wait expressions satisfy the scoped regression')
"#;

#[test]
fn live_browser_health_waits_bind_the_current_server_process() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let output = Command::new("python3")
        .args(["-B", "-c", AST_REGRESSION])
        .arg(root)
        .output()
        .expect("Python 3 is needed to inspect the legacy harness AST");
    assert!(
        output.status.success(),
        "live browser health AST regression failed (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
