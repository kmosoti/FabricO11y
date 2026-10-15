"""Opt-in R3 lifecycle control plane, entirely inside the bounded service."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import ctypes

from resource_final_group import FIELDS, require, show, snapshot, validate_empty


def identity(pid):
    process = Path('/proc') / str(pid)
    return {'pid': pid, 'start_ticks': int((process / 'stat').read_text().rsplit(')', 1)[1].split()[19]),
            'boot_id': Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
            'cgroup': (process / 'cgroup').read_text()}


def validate_inventory(outer, workload_name, guardian_pid):
    outside = {name: fields['cgroup.procs'].split() for name, fields in outer['groups'].items()
               if name != workload_name and not name.startswith(workload_name + '/')}
    require(outside.get('.') == [] and outside.get('control-plane') == [str(guardian_pid)]
            and all(not pids for name, pids in outside.items() if name != 'control-plane'),
            'actual task escaped the workload boundary')


def validate_observation(report):
    require(report['state'] == 'observed_workload_empty'
            and report['wrapper_wait_exit'] == report['command_receipt']['exit']
            and report['wrapper_reaped'] is True and report['descendants_echild'] is True,
            'wrapper/descendants not independently completed/reaped')
    require(report['wrapper_identity']['pid'] != report['guardian_identity']['pid']
            and all(type(record.get(key)) is int and record[key] > 0
                    for record in (report['wrapper_identity'], report['guardian_identity'])
                    for key in ('pid', 'start_ticks'))
            and report['wrapper_identity']['boot_id'] == report['guardian_identity']['boot_id']
            and report['systemd']['MainPID'] == str(report['guardian_identity']['pid'])
            and report['systemd']['ActiveState'] == 'active'
            and report['systemd']['SubState'] == 'running'
            and report['activation_monotonic_ns'] > 0, 'actual guardian/unit identity mismatch')
    require(report['command_receipt']['cgroup'] == report['workload_cgroup']
            and report['command_receipt']['state'] == 'completed', 'wrapper receipt workload mismatch')
    validate_empty(report['workload']['groups'])
    validate_inventory(report['outer'], 'workload', report['guardian_identity']['pid'])
    outer = report['outer']['groups']['.']
    require(all(outer[key] == report['workload']['groups']['.'][key] for key in
                ('memory.high', 'memory.max', 'memory.swap.max', 'pids.max', 'cpu.max')),
            'workload/outer caps differ')
    for fields in report['outer']['groups'].values():
        require(all(type(fields.get(key)) is str for key in FIELDS), 'incomplete outer field set')
        events = dict(line.split() for line in fields['memory.events'].splitlines())
        require(events.get('oom') == events.get('oom_kill') == '0'
                and fields['memory.swap.current'] == '0', 'dirty outer accounting')
    require(0 <= int(outer['memory.peak']) <= 6 * 1024**3
            and 0 <= int(outer['pids.peak']) <= 1536, 'outer aggregate bound exceeded')
    return True


def read_receipt(path, unit, command, workload):
    try:
        require(ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) == 0,
                'guardian subreaper unavailable')
        value = json.loads(path.read_text())
        if (value['unit'] == unit and value['command'] == command
                and value['cgroup'] == str(workload) and value['state'] == 'completed'
                and type(value['exit']) is int and 0 <= value['exit'] <= 255):
            return value
    except (OSError, ValueError, TypeError, KeyError):
        pass
    return None


def run(unit, receipt_path, command):
    from resource_group import require_limits, write_command_receipt
    outer = require_limits()
    require(outer.name == unit + '.service', 'guardian is not in actual owned service')
    report_path = receipt_path.with_name('.resource-guardian.json')
    fault = os.environ.get('FABRIC_GUARDIAN_CONTROL')
    report = {'state': 'started', 'unit': unit, 'accounting_timing': 'after_workload_reap_before_guardian_exit',
              'control_only': fault is not None, 'control': fault}
    outer_fd = workload_fd = gate_read = gate_write = None
    wrapper = None
    try:
        outer_fd = os.open(outer, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        control = outer / 'control-plane'
        control.mkdir()
        (control / 'cgroup.procs').write_text(str(os.getpid()))
        (outer / 'cgroup.subtree_control').write_text('+memory +cpu +pids +io')
        workload = outer / 'workload'
        workload.mkdir()
        for key in ('memory.high', 'memory.max', 'memory.swap.max', 'pids.max', 'cpu.max'):
            (workload / key).write_text((outer / key).read_text())
        workload_fd = os.open(workload, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        admission = snapshot(workload, workload_fd)
        validate_empty(admission['groups'])
        report['workload_admission'] = admission
        guardian = identity(os.getpid())
        state = show(unit, 5)
        require(state['MainPID'] == str(os.getpid()) and state['ControlGroup'] == str(outer).removeprefix('/sys/fs/cgroup'),
                'systemd does not independently identify guardian')
        anchor = int(state['ActiveEnterTimestampMonotonic']) * 1000
        require(anchor > 0, 'missing actual activation anchor')
        gate_read, gate_write = os.pipe()
        environment = dict(os.environ, FABRIC_WORKLOAD_ROOT=str(workload),
                           FABRIC_WORKLOAD_FD=str(workload_fd), FABRIC_RESOURCE_UNIT=unit,
                           FABRIC_RESOURCE_TOPOLOGY='guardian-v1')
        def enter():
            (workload / 'cgroup.procs').write_text(str(os.getpid()))
        wrapper = subprocess.Popen([sys.executable, str(Path(__file__).with_name('resource_group.py')),
            '--guarded-inside', str(gate_read), unit, str(receipt_path), *command],
            env=environment, pass_fds=(gate_read, workload_fd), preexec_fn=enter)
        wrapper_identity = identity(wrapper.pid)
        require(wrapper_identity['cgroup'] == '0::' + str(workload).removeprefix('/sys/fs/cgroup') + '\n',
                'actual wrapper did not enter held workload')
        report.update(guardian_identity=guardian, wrapper_identity=wrapper_identity,
                      systemd=state, activation_monotonic_ns=anchor, workload_cgroup=str(workload),
                      outer_cgroup=str(outer))
        os.close(gate_read)
        gate_read = None
        os.write(gate_write, b'1')
        os.close(gate_write)
        gate_write = None
        timeout = anchor / 1e9 + 7500 - time.monotonic()
        if fault == 'unexited':
            timeout = min(timeout, 3)
        require(timeout > 0, 'guardian activation deadline exhausted')
        status = wrapper.wait(timeout=timeout)
        descendants_echild = False
        while True:
            try:
                reaped, _ = os.waitpid(-1, os.WNOHANG)
                if reaped == 0:
                    break
            except ChildProcessError:
                descendants_echild = True
                break
        child = read_receipt(receipt_path, unit, command, workload)
        require(child is not None and status == child['exit'], 'actual wrapper exit/receipt disagreement')
        if fault == 'wrong-identity':
            workload.rmdir()
            workload.mkdir()
        before = time.monotonic_ns()
        report.update(wrapper_wait_exit=status, wrapper_reaped=True, descendants_echild=descendants_echild, command_receipt=child,
                      before_ns=before, workload=snapshot(workload, workload_fd), outer=snapshot(outer, outer_fd),
                      after_ns=time.monotonic_ns(), state='observed_workload_empty')
        require(show(unit, 5) == state and identity(os.getpid()) == guardian,
                'guardian/unit identity changed during observation')
        validate_inventory(report['outer'], 'workload', guardian['pid'])
        require((report['workload']['device'], report['workload']['inode']) ==
                (admission['device'], admission['inode']), 'workload held identity changed')
        validate_observation(report)
        write_command_receipt(report_path, report)
        return status
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        report.update(state='failed', error=repr(error))
        write_command_receipt(report_path, report)
        return 2
    finally:
        for descriptor in (outer_fd, workload_fd, gate_read, gate_write):
            if descriptor is not None:
                os.close(descriptor)
