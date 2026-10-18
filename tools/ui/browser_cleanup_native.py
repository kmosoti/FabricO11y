"""Finite real-kernel cleanup controls; no browser or product acceptance claim."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time

import live_browser_check as browser

CHILD = r'''
import os, signal, sys, time
from pathlib import Path
root, mode = Path(sys.argv[1]), sys.argv[2]
end = time.monotonic() + 45
def mark(name, value):
    temporary = root / (name + '.tmp')
    temporary.write_text(str(value)); temporary.replace(root / name)
while not (root / 'go').exists():
    if time.monotonic() > end: os._exit(2)
    time.sleep(.01)
child = os.fork()
if child == 0:
    if mode == 'double':
        grandchild = os.fork()
        if grandchild:
            mark('grandchild', grandchild)
            os._exit(0)
        os.setsid()
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    mark('subject', os.getpid())
    if mode == 'zombie': os._exit(17)
    while time.monotonic() < end: time.sleep(.02)
    os._exit(0)
mark('child', child)
while not (root / 'subject').exists():
    if time.monotonic() > end: os._exit(3)
    time.sleep(.01)
if mode in ('double', 'zombie'): os._exit(0)
while time.monotonic() < end: time.sleep(.02)
'''


def observed(pid):
    raw = (Path('/proc') / str(pid) / 'stat').read_text()
    fields = raw.rsplit(')', 1)[1].split()
    return dict(pid=pid, start_ticks=int(fields[19]), state=fields[0], ppid=int(fields[1]),
                pgid=int(fields[2]), raw_stat=raw,
                raw_cgroup=(Path('/proc') / str(pid) / 'cgroup').read_text())


def wait_for(predicate, seconds=5):
    until = time.monotonic() + seconds
    while not predicate():
        if time.monotonic() >= until: raise RuntimeError('native fixture readiness timeout')
        time.sleep(.01)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--browser-entry-helper', type=Path, required=True)
    parser.add_argument('--browser-entry-helper-sha256', required=True)
    args = parser.parse_args()
    if (not args.browser_entry_helper.is_absolute()
            or args.browser_entry_helper.is_symlink()
            or not args.browser_entry_helper.is_file()
            or not os.access(args.browser_entry_helper, os.X_OK)
            or len(args.browser_entry_helper_sha256) != 64
            or any(char not in '0123456789abcdef' for char in args.browser_entry_helper_sha256)
            or hashlib.sha256(args.browser_entry_helper.read_bytes()).hexdigest()
                != args.browser_entry_helper_sha256):
        raise ValueError('exact pinned native browser entry helper required')
    limits = browser.require_limits()
    subprocess.run(['systemctl', '--user', 'set-property', '--runtime', limits.name,
        'MemoryHigh=3500000000', 'MemoryMax=4000000000', 'MemorySwapMax=0',
        'CPUQuota=200%', 'TasksMax=512'], check=True, timeout=15)
    out = args.out.absolute()
    assert out.parent.resolve(strict=True).is_relative_to(browser.DATA / 'results') and not out.exists()
    out.mkdir(mode=0o700)
    root = Path(tempfile.mkdtemp(prefix='browser-native-cleanup-', dir=os.environ['FABRIC_SCRATCH_ROOT']))
    parent = browser.begin_browser_domain(limits, time.monotonic() + 240)
    bindings, guardians, leaders, logs = [], [], [], []
    report = dict(scope='native Linux cgroup/process cleanup only', exit=1, cases=[],
                  parent=str(parent['path']), namespaces=parent['namespaces'])
    foreign = None
    try:
        for mode in ('normal', 'double', 'zombie', 'foreign'):
            cell = root / mode; cell.mkdir(mode=0o700)
            script = cell / 'child.py'; script.write_text(CHILD)
            log = (out / (mode + '.log')).open('x'); logs.append(log)
            proc = browser.browser_group_launch(parent, bindings, mode,
                [sys.executable, '-B', str(script), str(cell), 'normal' if mode == 'foreign' else mode],
                log, dict(os.environ), entry_helper=args.browser_entry_helper,
                entry_helper_sha256=args.browser_entry_helper_sha256)
            leaders.append((proc, os.pidfd_open(proc.pid), observed(proc.pid)['start_ticks']))
            bound = bindings[-1]['group']
            row = dict(case=mode, bound_pids=[p['pid'] for p in bound['members']],
                leader=observed(proc.pid), group=str(bound['path']), before=(bound['path'] / 'cgroup.events').read_text())
            report['cases'].append(row)
            (cell / 'go').write_text('start after binding\n')
            wait_for(lambda: (cell / 'subject').exists() and (cell / 'child').exists()
                     and (mode != 'double' or (cell / 'grandchild').exists()))
            ids = {int((cell / name).read_text()) for name in ('subject', 'child', 'grandchild') if (cell / name).exists()}
            row['original_children'] = []
            for pid in sorted(ids):
                before = observed(pid); descriptor = os.pidfd_open(pid)
                assert observed(pid)['start_ticks'] == before['start_ticks']
                guardians.append((pid, before['start_ticks'], descriptor))
                row['original_children'].append(before)
            subject = int((cell / 'subject').read_text())
            assert subject not in row['bound_pids'], 'fixture must fork only after binding'
            if mode in ('double', 'zombie'):
                proc.wait(timeout=5)
                if mode == 'double': assert observed(subject)['pgid'] == subject
                else:
                    wait_for(lambda: observed(subject)['state'] == 'Z')
                    assert str(subject) not in (bound['path'] / 'cgroup.procs').read_text().split()
            if mode == 'foreign':
                foreign = subprocess.Popen([sys.executable, '-B', '-c', 'raise SystemExit(23)'])
                wait_for(lambda: observed(foreign.pid)['state'] == 'Z')
                row['foreign_before'] = observed(foreign.pid)
            browser.stop_browser_groups(parent, [bindings[-1]])
            try:
                browser.browser_reap_domain(parent, bindings)
            except RuntimeError as error:
                assert mode == 'foreign' and str(error) == 'browser-group-foreign-child', str(error)
                assert observed(foreign.pid)['start_ticks'] == row['foreign_before']['start_ticks']
                row['foreign_after_refusal'] = observed(foreign.pid)
                assert foreign.wait(timeout=1) == 23
                foreign = None
                browser.browser_reap_domain(parent, bindings)
            else:
                assert mode != 'foreign', 'foreign child was silently consumed'
            row['after'] = (bound['path'] / 'cgroup.events').read_text()
            assert 'populated 0' in row['after'].splitlines()
            assert all(not (Path('/proc') / str(pid)).exists() for pid in ids)
            assert not (Path('/proc') / str(proc.pid)).exists()
            row['original_children_absent'] = True
            row['own_children_empty'] = parent['own_children_empty']
            row['late_reaped'] = bound.get('late_reaped', [])
            row['exit'] = 0
        browser.end_browser_domain(parent, bindings)
        report['exit'] = 0
    except BaseException as error:
        report['error'] = type(error).__name__ + ': ' + str(error)
    finally:
        # Independent guardian signals and reaps only its separately held identities.
        guardian_errors = []
        if foreign is not None:
            leaders.append((foreign, os.pidfd_open(foreign.pid), observed(foreign.pid)['start_ticks']))
        for proc, descriptor, start in leaders:
            try:
                if (Path('/proc') / str(proc.pid)).exists():
                    assert observed(proc.pid)['start_ticks'] == start
                    signal.pidfd_send_signal(descriptor, signal.SIGKILL)
            except BaseException as error: guardian_errors.append(str(error))
        for pid, start, descriptor in guardians:
            try:
                if (Path('/proc') / str(pid)).exists():
                    assert observed(pid)['start_ticks'] == start
                    signal.pidfd_send_signal(descriptor, signal.SIGKILL)
            except BaseException as error: guardian_errors.append(str(error))
        for proc, descriptor, start in leaders:
            try: proc.wait(timeout=5)
            except BaseException as error: guardian_errors.append(str(error))
            finally: os.close(descriptor)
        for pid, start, descriptor in guardians:
            try:
                until = time.monotonic() + 5
                while (Path('/proc') / str(pid)).exists():
                    assert observed(pid)['start_ticks'] == start
                    try: os.waitid(os.P_PIDFD, descriptor, os.WEXITED | os.WNOHANG | 0x40000000)
                    except ChildProcessError: pass
                    if time.monotonic() >= until: raise RuntimeError('guardian reap timeout')
                    time.sleep(.01)
            except BaseException as error: guardian_errors.append(str(error))
            finally: os.close(descriptor)
        for log in logs: log.close()
        report['guardian_errors'] = guardian_errors
        if guardian_errors: report['exit'] = 1
        if report['exit'] == 0: shutil.rmtree(root)
        else: report['retained_failure_fixture'] = str(root)
        (out / 'receipt.json').write_text(json.dumps(report, indent=2) + '\n')
    return report['exit']


if __name__ == '__main__':
    sys.exit(main())
