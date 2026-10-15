"""Delegated supervisor lifecycle controls with an independent child guardian."""
import argparse
import ctypes
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time

import browser_supervisor as supervisor
from browser_cleanup_native import observed, wait_for

DRIVER = r'''
import os, signal, sys, time
from pathlib import Path
root, mode = Path(sys.argv[1]), sys.argv[2]
end = time.monotonic() + 120
while not (root / 'go').exists():
    if time.monotonic() >= end: os._exit(2)
    time.sleep(.01)
child = os.fork()
if child == 0:
    os.setsid()
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    temporary = root / 'subject.tmp'
    temporary.write_text(str(os.getpid())); temporary.replace(root / 'subject')
    if mode == 'zombie': os._exit(17)
    while time.monotonic() < end: time.sleep(.02)
    os._exit(0)
while time.monotonic() < end: time.sleep(.02)
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    browser = supervisor.browser
    limits = browser.require_limits()
    subprocess.run(['systemctl', '--user', 'set-property', '--runtime', limits.name,
                    'MemoryHigh=5G', 'MemoryMax=6G', 'MemorySwapMax=0', 'CPUQuota=200%', 'TasksMax=1536'], check=True, timeout=15)
    require = supervisor.require
    require(not args.out.exists() and args.out.parent.resolve().is_relative_to(browser.DATA / 'results'), 'fixture-output')
    args.out.mkdir(mode=0o700)
    root = Path(tempfile.mkdtemp(prefix='browser-supervisor-native-', dir=os.environ['FABRIC_SCRATCH_ROOT']))
    try:
        os.waitid(os.P_ALL, 0, os.WEXITED | os.WNOHANG | os.WNOWAIT | 0x40000000)
    except ChildProcessError:
        pass
    else:
        raise RuntimeError('guardian starts with foreign children')
    libc, previous = ctypes.CDLL(None), ctypes.c_int()
    require(libc.prctl(37, ctypes.byref(previous), 0, 0, 0) == 0
            and libc.prctl(36, 1, 0, 0, 0) == 0, 'fixture-subreaper')
    browser.cgroups.delegate()
    browser.cgroups.set_limits(limits / 'supervisor', 6 * 2**30, 5 * 2**30, 1536, 2)
    parent = browser.browser_hold_directory(limits)
    parent.update(namespaces=browser.browser_namespaces(), deadline_monotonic_ns=time.monotonic_ns() + 240 * 10**9)
    report = dict(classification='native supervisor lifecycle only; no browser or package acceptance', exit=1, cases=[])
    all_empty = False
    try:
        for mode in ('direct', 'normal', 'refusal', 'before-ready', 'during-cleanup'):
            cell = root / mode
            cell.mkdir(mode=0o700)
            script = cell / 'driver.py'
            script.write_text(DRIVER)
            command = ['/nonexistent/fabric-owned-driver'] if mode == 'refusal' else [sys.executable, '-B', str(script), str(cell), 'normal' if mode in ('normal', 'direct') else 'zombie']
            instance = supervisor.BrowserSupervisor.__new__(supervisor.BrowserSupervisor)
            errors, guardians = [], []
            held = worker = None
            row = dict(case=mode, exit=1)
            report['cases'].append(row)
            log = (args.out / (mode + '.log')).open('xb')
            budget = None
            if mode != 'direct':
                budget_path, _ = browser.cgroups.subgroup(limits, 'native-browser-budget-' + mode,
                    2**30, 850 * 2**20, 512, 2)
                budget = browser.browser_hold_directory(budget_path)
                budget['exclusive'] = True
                row['budget_limits'] = supervisor.budget_limits(budget)
            def initialize():
                try:
                    instance.__init__(command, dict(os.environ), log, parent['deadline_monotonic_ns'] / 1e9,
                        args.out / (mode + '-supervisor'), fault=mode if mode in ('before-ready', 'during-cleanup') else None,
                        browser_parent=budget)
                except BaseException as error:
                    errors.append(type(error).__name__ + ': ' + str(error))
            def close():
                try: instance.close(deadline=time.monotonic() + 20)
                except BaseException as error: errors.append(type(error).__name__ + ': ' + str(error))
            def capture(pid):
                before = observed(pid)
                descriptor = os.pidfd_open(pid)
                require(observed(pid)['start_ticks'] == before['start_ticks'], 'guardian-identity')
                guardians.append((pid, before['start_ticks'], descriptor))
                row.setdefault('original_processes', []).append(before)
                return descriptor
            try:
                if mode == 'before-ready':
                    worker = threading.Thread(target=initialize)
                    worker.start()
                    marker = args.out / (mode + '-supervisor/fault-before-ready.json')
                    wait_for(marker.exists)
                    require(json.loads(marker.read_text())['pid'] == instance.process.pid, 'fixture-marker')
                else:
                    initialize()
                    if mode == 'refusal' and not errors:
                        wait_for(lambda: instance.poll() is not None)
                        close()
                require(instance.domain is not None, 'fixture-domain')
                held = browser.browser_hold_directory(instance.domain['path'])
                require((held['dev'], held['ino']) == (instance.domain['dev'], instance.domain['ino']), 'guardian-directory')
                if mode != 'refusal':
                    manager_fd = capture(instance.process.pid)
                    (cell / 'go').write_text('fork only after supervisor binding\n')
                    wait_for(lambda: (cell / 'subject').exists())
                    child = int((cell / 'subject').read_text())
                    capture(child)
                    if mode not in ('normal', 'direct'):
                        wait_for(lambda: observed(child)['state'] == 'Z')
                        require(str(child) not in (held['path'] / 'cgroup.procs').read_text().split(), 'fixture-zombie')
                    if mode == 'during-cleanup':
                        worker = threading.Thread(target=close)
                        worker.start()
                        marker = args.out / (mode + '-supervisor/fault-during-cleanup.json')
                        wait_for(marker.exists)
                        require(json.loads(marker.read_text())['pid'] == instance.process.pid, 'fixture-marker')
                    if mode in ('before-ready', 'during-cleanup'):
                        wait_for(lambda: observed(instance.process.pid)['state'] == 'T')
                        signal.pidfd_send_signal(manager_fd, signal.SIGKILL)
                        worker.join(timeout=5)
                        require(not worker.is_alive() and errors and not instance.cleanup_confirmed, 'fixture-refusal')
                    else:
                        close()
                        require(not errors and instance.cleanup_confirmed, 'fixture-positive')
                else:
                    require(errors and not instance.cleanup_confirmed, 'fixture-startup-refusal')
                row['observed_errors'] = list(errors)
                row['browser_cleanup_confirmed'] = instance.cleanup_confirmed
            finally:
                guardian_errors = []
                try:
                    if held is None and getattr(instance, 'domain', None) is not None:
                        held = browser.browser_hold_directory(instance.domain['path'])
                    if held is not None:
                        browser.browser_directory(held)
                        descriptor = os.open('cgroup.kill', os.O_WRONLY | os.O_NOFOLLOW, dir_fd=held['fd'])
                        try: os.write(descriptor, b'1')
                        finally: os.close(descriptor)
                    if worker is not None:
                        worker.join(timeout=5)
                        require(not worker.is_alive(), 'guardian-worker')
                    if getattr(instance, 'process', None) is not None:
                        instance.process.wait(timeout=5)
                    if held is not None:
                        browser.browser_reap_domain(parent, [{'group': held}])
                        row['population'] = (held['path'] / 'cgroup.events').read_text()
                        require('populated 0' in row['population'].splitlines(), 'guardian-population')
                        row['guardian_late_reaped'] = held.get('late_reaped', [])
                    require(parent.get('own_children_empty') is True, 'guardian-wait-domain')
                    require(all(not (Path('/proc') / str(pid)).exists() for pid, _, _ in guardians), 'guardian-absence')
                    row.update(guardian_own_children_empty=True, original_processes_absent=True)
                except BaseException as error:
                    guardian_errors.append(type(error).__name__ + ': ' + str(error))
                finally:
                    for _, _, descriptor in guardians: os.close(descriptor)
                    if held is not None: os.close(held['fd'])
                    if budget is not None: os.close(budget['fd'])
                    log.close()
                    row['guardian_errors'] = guardian_errors
                require(not guardian_errors, 'guardian-failed')
            row['exit'] = 0
        for defect in ('wrong-ancestor', 'changed-ancestor'):
            budget_path, _ = browser.cgroups.subgroup(limits, 'native-browser-budget-' + defect,
                2**30, 850 * 2**20, 512, 2)
            budget = browser.browser_hold_directory(budget_path)
            budget['exclusive'] = True
            supplied = dict(parent, exclusive=True) if defect == 'wrong-ancestor' else dict(budget, ino=budget['ino'] + 1)
            row = dict(case=defect, exit=1)
            report['cases'].append(row)
            before = set(os.listdir('/proc/self/fd'))
            instance = supervisor.BrowserSupervisor.__new__(supervisor.BrowserSupervisor)
            try:
                try:
                    instance.__init__([sys.executable, '-B', '-c', 'raise SystemExit(99)'], dict(os.environ), None,
                        parent['deadline_monotonic_ns'] / 1e9, args.out / defect, browser_parent=supplied)
                except RuntimeError as error:
                    require(str(error) == ('browser-supervisor-budget-ancestor' if defect == 'wrong-ancestor'
                                          else 'browser-group-identity'), 'fixture-refusal-diagnostic')
                    row['refusal'] = str(error)
                else:
                    raise RuntimeError('invalid browser ancestor admitted')
                require(instance.process is None and instance.domain is None, 'fixture-no-launch')
                require(set(os.listdir('/proc/self/fd')) == before, 'fixture-descriptor-leak')
                browser.browser_reap_domain(parent, [])
                require(parent['own_children_empty'], 'fixture-refusal-wait-domain')
                row.update(exit=0, no_launch=True, descriptor_set_unchanged=True, own_children_empty=True)
            finally:
                os.close(budget['fd'])
        all_empty = True
        report['exit'] = 0
    except BaseException as error:
        report['error'] = type(error).__name__ + ': ' + str(error)
    finally:
        if all_empty:
            if libc.prctl(36, previous.value, 0, 0, 0) != 0:
                report.update(exit=1, restore_error='fixture-subreaper-restore')
            else:
                shutil.rmtree(root)
        else:
            report['retained_failure_fixture'] = str(root)
        os.close(parent['fd'])
        (args.out / 'receipt.json').write_text(json.dumps(report, indent=2) + '\n')
    return report['exit']


if __name__ == '__main__':
    raise SystemExit(main())
