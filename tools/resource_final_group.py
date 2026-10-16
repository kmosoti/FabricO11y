"""Opt-in R3 outside-launcher observation of the retained empty service."""
import os
from pathlib import Path
import subprocess
import signal
import time

FIELDS = ('memory.high', 'memory.max', 'memory.swap.max', 'pids.max', 'memory.events',
    'memory.peak', 'memory.current', 'memory.swap.current', 'pids.current', 'pids.peak',
    'cgroup.events', 'cgroup.procs', 'cpu.max', 'cpu.stat', 'io.stat')


def require(value, message):
    if not value:
        raise ValueError(message)


def options(arguments):
    arguments = list(arguments)
    selected = arguments[:1] == ['--observe-final-group']
    return selected, arguments[1:] if selected else arguments


def validate_empty(groups):
    require(type(groups) is dict and '.' in groups, 'missing final whole-parent kernel observation')
    for name, group in groups.items():
        require(type(name) is str and type(group) is dict and all(type(group.get(key)) is str for key in FIELDS),
                'missing final parent/subgroup field')
        events = dict(line.split() for line in group['memory.events'].splitlines())
        population = dict(line.split() for line in group['cgroup.events'].splitlines())
        require(events.get('oom') == events.get('oom_kill') == '0'
                and group['memory.swap.current'] == '0' and population.get('populated') == '0'
                and not group['cgroup.procs'].strip() and group['pids.current'] == '0',
                'final parent/subgroup is not clean and empty')
    parent = groups['.']
    require(all(parent[key] == value for key, value in {'memory.high': '5368709120',
        'memory.max': '6442450944', 'memory.swap.max': '0', 'pids.max': '1536'}.items()),
        'final whole-parent cap mismatch')
    require(all(0 <= int(parent[key]) <= maximum for key, maximum in
        [('memory.current', 6442450944), ('memory.peak', 6442450944), ('pids.peak', 1536)]),
        'final whole-parent charge/task bound')
    return True


def show(unit, timeout):
    result = subprocess.run(['systemctl', '--user', 'show', unit, '--no-pager',
        '--property=ActiveState,SubState,MainPID,ActiveEnterTimestampMonotonic,ControlGroup,ExecMainStatus,Result'],
        check=True, capture_output=True, text=True, timeout=timeout)
    require(len(result.stdout) <= 65536, 'oversized owned unit observation')
    values = dict(line.split('=', 1) for line in result.stdout.splitlines())
    require(set(values) == {'ActiveState', 'SubState', 'MainPID', 'ActiveEnterTimestampMonotonic',
                           'ControlGroup', 'ExecMainStatus', 'Result'}, 'incomplete owned unit observation')
    return values


def snapshot(group, descriptor):
    identity = lambda info: (info.st_dev, info.st_ino)
    held = identity(os.fstat(descriptor))
    require(identity(group.stat()) == held, 'final cgroup directory identity changed')
    paths = [group, *(path for path in group.rglob('*') if path.is_dir())]
    identities = {str(path.relative_to(group)): identity(path.stat()) for path in paths}
    groups = {str(path.relative_to(group)): {key: (path / key).read_text().strip() for key in FIELDS}
              for path in paths}
    require(identity(group.stat()) == held == identity(os.fstat(descriptor)), 'final cgroup replaced during read')
    require(set(groups) == {str(path.relative_to(group)) for path in
                          [group, *(path for path in group.rglob('*') if path.is_dir())]},
            'final subgroup set changed during read')
    require(identities == {str(path.relative_to(group)): identity(path.stat()) for path in paths},
            'final subgroup directory identity changed')
    return {'device': held[0], 'inode': held[1], 'groups': groups, 'subgroup_identities': identities}


def observe_service(command, receipt_path, unit, arguments, runtime, read_receipt):
    """Hold the exited service until real empty fields are read, then stop it.

    Returns the real systemd-run exit plus a raw observation/error record.
    Failure always remains failure even if forced cleanup later empties it.
    """
    deadline = time.monotonic() + runtime
    report = {'state': 'observing', 'unit': unit, 'observations': [], 'forced_cleanup': False}
    def expired(_number, _frame):
        raise TimeoutError('R3 outside observation deadline exhausted')
    previous_handler = signal.signal(signal.SIGALRM, expired)
    signal.setitimer(signal.ITIMER_REAL, runtime)
    transport = None
    descriptor = None
    group = None
    def remaining(limit=5):
        left = deadline - time.monotonic()
        require(left > 0, 'R3 final observation global deadline exhausted')
        return min(left, limit)
    try:
        transport = subprocess.Popen(command)
        while True:
            remaining()
            child = read_receipt(receipt_path, unit, arguments)
            if child is None or child.get('state') != 'completed':
                require(transport.poll() is None, 'transport exited without complete child receipt')
                time.sleep(min(.05, remaining()))
                continue
            observed = show(unit, remaining())
            anchor = int(observed['ActiveEnterTimestampMonotonic']) * 1000
            require(anchor > 0, 'missing actual service activation anchor')
            deadline = min(deadline, anchor / 1_000_000_000 + runtime)
            signal.setitimer(signal.ITIMER_REAL, remaining(runtime))
            expected_group = Path('/sys/fs/cgroup') / observed['ControlGroup'].lstrip('/')
            require(observed['ControlGroup'].startswith('/') and '..' not in expected_group.parts
                    and expected_group.name == unit + '.service' and str(expected_group) == child['cgroup'],
                    'different unit/cgroup child receipt')
            require(observed['ActiveState'] == 'active' and observed['Result'] == 'success',
                    'failed or inactive retained unit')
            if observed['MainPID'] != '0' or observed['SubState'] != 'exited':
                require(transport.poll() is None, 'transport exited while inside wrapper was live')
                time.sleep(min(.01, remaining()))
                continue
            require(type(child.get('exit')) is int and child['exit'] == int(observed['ExecMainStatus']),
                    'actual inside exit differs from child receipt')
            group = expected_group
            descriptor = os.open(group, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
            before = time.monotonic_ns()
            fields = snapshot(group, descriptor)
            validate_empty(fields['groups'])
            confirmed = show(unit, remaining())
            require(confirmed == observed, 'unit identity/state changed during final read')
            report.update(state='observed_empty', activation_monotonic_ns=anchor,
                activation_uncertainty_ns=1000, cgroup=str(group), before_ns=before,
                after_ns=time.monotonic_ns(), systemd=confirmed, **fields)
            remaining()
            stopped = subprocess.run(['systemctl', '--user', 'stop', unit], capture_output=True,
                                     text=True, timeout=remaining())
            report['stop_exit'] = stopped.returncode
            require(stopped.returncode == 0, 'retained unit stop failed')
            status = transport.wait(timeout=remaining())
            require(status == child['exit'], 'transport exit differs from actual inside exit')
            return status, report
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        signal.setitimer(signal.ITIMER_REAL, 0)
        report.update(state='failed', error=repr(error), forced_cleanup=True)
        try:
            stopped = subprocess.run(['systemctl', '--user', 'stop', unit], capture_output=True,
                                     text=True, timeout=5)
            report['failure_stop_exit'] = stopped.returncode
        except (OSError, subprocess.SubprocessError) as stop_error:
            report['failure_stop_error'] = repr(stop_error)
        status = None
        if transport is not None:
            try:
                status = transport.wait(timeout=5)
            except subprocess.TimeoutExpired:
                transport.kill()
                status = transport.wait(timeout=5)
                report['transport_forced_kill'] = True
        return status, report
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous_handler)
        if descriptor is not None:
            os.close(descriptor)
