use std::process::Command;

const LINUX_FIXTURE: &str = r#"
import atexit, hashlib, importlib.util, json, os, pathlib, resource, secrets, signal, sys, time, subprocess
root = pathlib.Path(os.environ['FABRIC_REPO_ROOT'])
sys.path.insert(0, str(root / 'tools/ui'))
script = root / 'tools/ui/live_browser_check.py'
spec = importlib.util.spec_from_file_location('live_browser_check_fixture', script)
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)
helper = pathlib.Path(os.environ['FABRIC_BROWSER_ENTRY_HELPER'])
helper_sha = hashlib.sha256(helper.read_bytes()).hexdigest()
limits = module.require_limits()
scratch = pathlib.Path(os.environ['FABRIC_SCRATCH_ROOT'])
fixture = scratch / ('browser-entry-' + secrets.token_hex(8))
fixture.mkdir(mode=0o700)
name = 'browser-entry-' + secrets.token_hex(6)
parent = None
bindings = []
domain_closed = False

def empty(path):
    return 'populated 0' in (path / 'cgroup.events').read_text().splitlines()

def subgroup(parent, label):
    path, actual = module.cgroups.subgroup(parent['path'], label, 128*1024*1024,
                                            64*1024*1024, 16, 1)
    record = module.browser_hold_directory(path)
    record.update(exclusive=True)
    return path, record

def launch_raw(path, timeout_ms, label):
    ready_r, ready_w = os.pipe()
    release_r, release_w = os.pipe()
    log = (fixture / (label + '.log')).open('wb')
    command = [str(helper), 'browser-enter-exec', '--group', str(path),
               '--ready-fd', str(ready_w), '--release-fd', str(release_r),
               '--timeout-ms', str(timeout_ms), '--', '/usr/bin/true']
    proc = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT,
                            start_new_session=True, pass_fds=(ready_w, release_r))
    os.close(ready_w); os.close(release_r)
    pidfd = os.pidfd_open(proc.pid)
    start = module.browser_pid(proc.pid)[0]
    group = module.browser_hold_directory(path)
    group.update(exclusive=True, process=proc, pid=proc.pid, pidfd=pidfd, start_ticks=start)
    record = {'parent': parent, 'group': group, 'state': 'spawned', 'process': proc}
    parent.setdefault('provisional_children', []).append(record)
    poller = module.select.poll(); poller.register(ready_r, module.select.POLLIN | module.select.POLLHUP)
    events = poller.poll(5000)
    ready = os.read(ready_r, 2) if events and events[0][1] & module.select.POLLIN else b''
    os.close(ready_r)
    if ready != b'R':
        os.close(release_w)
        try: module.signal.pidfd_send_signal(pidfd, module.signal.SIGKILL)
        except ProcessLookupError: pass
        proc.wait(timeout=5); module.browser_provisional_cleanup(parent, record); log.close()
        raise AssertionError(label + ': native helper did not publish READY')
    record['ready_observed'] = True
    record['release_writer'] = release_w
    record['log'] = log
    return record

def assert_stopped(path, record):
    assert empty(path), str(path) + ': subgroup remained populated'
    module.browser_provisional_cleanup(parent, record)

def emergency_cleanup():
    if parent is None or domain_closed:
        return
    for record in parent.get('provisional_children', []):
        writer = record.pop('release_writer', None)
        if writer is not None:
            try: os.close(writer)
            except OSError: pass
    try:
        module.browser_cleanup_provisionals(parent)
        module.stop_browser_groups(parent, bindings)
        module.browser_reap_domain(parent, bindings)
        module.end_browser_domain(parent, bindings)
    except BaseException as error:
        print('integration fixture cleanup failed: ' + str(error), file=sys.stderr)

atexit.register(emergency_cleanup)

# Preserve a deliberate zombie before domain admission. It must still be refused.
foreign = subprocess.Popen(['/usr/bin/true'])
foreign_fd = os.pidfd_open(foreign.pid)
until = time.monotonic() + 2
while time.monotonic() < until:
    if os.waitid(os.P_PIDFD, foreign_fd, os.WEXITED | os.WNOHANG | os.WNOWAIT):
        break
    time.sleep(.005)
else:
    raise AssertionError('foreign-child control did not become a zombie')
try:
    module.begin_browser_domain(limits, time.monotonic() + 30)
    raise AssertionError('foreign-child guard admitted an unregistered zombie')
except RuntimeError as error:
    assert str(error) == 'browser-group-foreign-child', str(error)
finally:
    foreign.wait(timeout=2); os.close(foreign_fd)

parent = module.begin_browser_domain(limits, time.monotonic() + 45)
bindings = []
started = time.monotonic_ns()
with (fixture / 'positive.log').open('wb') as log:
    proc = module.browser_group_launch(parent, bindings, 'immediate-true',
        ['/usr/bin/true'], log, os.environ.copy(), entry_helper=helper,
        entry_helper_sha256=helper_sha)
    assert len(bindings) == 1 and proc is bindings[0]['group']['process']
    binding = bindings[0]
    assert binding['group'].get('entry_helper_sha256') == helper_sha
    assert proc.wait(timeout=5) == 0
module.stop_browser_groups(parent, bindings)
assert bindings[0]['group']['kernel_empty'] and bindings[0]['group']['children_reaped']
positive_peak = int((bindings[0]['group']['path'] / 'memory.peak').read_text())

# Alter only the child group argv while running the actual parent launch seam.
wrong_path, wrong = subgroup(parent, name + '-wrong')
marker = fixture / 'wrong-placement-target-executed'
original_popen = module.subprocess.Popen
class WrongPlacementPopen(original_popen):
    def __init__(self, argv, *args, **kwargs):
        argv = list(argv)
        argv[argv.index('--group') + 1] = str(wrong_path)
        super().__init__(argv, *args, **kwargs)
module.subprocess.Popen = WrongPlacementPopen
try:
    with (fixture / 'wrong-placement.log').open('wb') as log:
        try:
            module.browser_group_launch(parent, bindings, 'wrong-placement',
                ['/usr/bin/touch', str(marker)], log, os.environ.copy(),
                entry_helper=helper, entry_helper_sha256=helper_sha)
            raise AssertionError('wrong-placement control was accepted')
        except RuntimeError as error:
            assert str(error) == 'browser-group-placement', str(error)
finally:
    module.subprocess.Popen = original_popen
wrong_record = parent['provisional_children'][-1]
assert wrong_record['state'] == 'cleaned' and wrong_record.get('children_reaped') is True
assert not marker.exists(), 'wrong-placement target executed before parent binding'
assert empty(wrong_path), 'wrong-placement sibling remained populated'
os.close(wrong['fd'])

# READY followed by EOF and a bounded release timeout must never execute target.
eof_path, _ = module.cgroups.subgroup(parent['path'], name + '-eof',
                                      128*1024*1024, 64*1024*1024, 16, 1)
eof_record = launch_raw(eof_path, 5000, 'release-eof')
proc = eof_record['process']
release_w, log = eof_record['release_writer'], eof_record['log']
os.close(release_w)
eof_record.pop('release_writer', None)
assert proc.wait(timeout=5) != 0
assert_stopped(eof_path, eof_record); log.close(); eof_record.pop('log', None)

timeout_path, _ = module.cgroups.subgroup(parent['path'], name + '-timeout',
                                          128*1024*1024, 64*1024*1024, 16, 1)
timeout_record = launch_raw(timeout_path, 100, 'release-timeout')
proc = timeout_record['process']
release_w, log = timeout_record['release_writer'], timeout_record['log']
assert proc.wait(timeout=2) != 0
os.close(release_w); timeout_record.pop('release_writer', None)
assert_stopped(timeout_path, timeout_record); log.close(); timeout_record.pop('log', None)

module.browser_reap_domain(parent, bindings)
module.end_browser_domain(parent, bindings)
domain_closed = True
elapsed = time.monotonic_ns() - started
print(json.dumps({'state':'complete','positive_exit':0,'positive_elapsed_ns':elapsed,
    'positive_peak_cgroup_bytes':positive_peak,
    'peak_child_rss_kib':resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss,
    'wrong_placement_rejected':True,'release_eof_rejected':True,
    'release_timeout_rejected':True,'foreign_child_rejected':True,
    'own_children_empty':parent.get('own_children_empty') is True}))
import shutil
shutil.rmtree(fixture)
"#;

/// Linux/cgroup composition control; intentionally excluded from ordinary workspace tests.
#[test]
#[ignore = "requires explicit DATA-backed resource_group admission and delegated cgroup v2"]
fn actual_parent_seam_binds_before_releasing_native_short_child() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    let helper = env!("CARGO_BIN_EXE_xtask");
    let result = Command::new("python3")
        .arg("-c")
        .arg(LINUX_FIXTURE)
        .env("FABRIC_REPO_ROOT", root)
        .env("FABRIC_BROWSER_ENTRY_HELPER", helper)
        .output()
        .expect("launch bounded Linux fixture");
    assert!(
        result.status.success(),
        "Linux fixture failed ({:?}): {}\n{}",
        result.status.code(),
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).expect("fixture JSON");
    assert_eq!(report["state"], "complete");
    assert_eq!(report["positive_exit"], 0);
    for key in [
        "wrong_placement_rejected",
        "release_eof_rejected",
        "release_timeout_rejected",
        "foreign_child_rejected",
        "own_children_empty",
    ] {
        assert_eq!(report[key], true, "missing integration control {key}");
    }
}
