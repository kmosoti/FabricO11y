#!/usr/bin/env python3
"""Run an entire project command tree in a bounded systemd user service."""
import os
from pathlib import Path
import subprocess
import sys
import json
import shutil
import tempfile
import time
import uuid

MAX_BYTES = 20 * 1024**3
DATA_DRIVE = Path('/run/media/kmosoti/data')
STORAGE = DATA_DRIVE / 'FabricO11y'
RECEIPTS = Path(__file__).resolve().parents[1] / 'target/resource-containment/runs'
DEFAULT_RUNTIME_SECONDS = 1800
MAX_RUNTIME_SECONDS = 16000


def command_options(arguments):
    """Explicit finite extension for owner-scoped long experiments only."""
    arguments = list(arguments)
    runtime = DEFAULT_RUNTIME_SECONDS
    delegated = False
    if arguments[:1] == ['--delegate']:
        delegated = True
        arguments = arguments[1:]
    if arguments[:1] == ['--runtime-seconds']:
        if len(arguments) < 2:
            raise ValueError('missing runtime seconds')
        runtime = int(arguments[1])
        arguments = arguments[2:]
        if not 0 < runtime <= MAX_RUNTIME_SECONDS:
            raise ValueError('runtime outside finite owner-scoped range')
    if arguments[:1] == ['--']:
        arguments = arguments[1:]
    if not arguments:
        raise ValueError('missing command')
    return runtime, delegated, arguments


def require_limits():
    """Fail closed unless our actual cgroup has memory and swap enforcement."""
    entries = Path('/proc/self/cgroup').read_text().splitlines()
    relative = next((line[3:] for line in entries if line.startswith('0::')), None)
    if relative is None:
        raise RuntimeError('unified cgroup v2 required')
    group = Path('/sys/fs/cgroup') / relative.lstrip('/')
    memory = (group / 'memory.max').read_text().strip()
    swap = (group / 'memory.swap.max').read_text().strip()
    if memory == 'max' or not 0 < int(memory) <= MAX_BYTES or swap != '0':
        raise RuntimeError(f'unsafe cgroup limits: memory.max={memory}, swap.max={swap}')
    print(f'resource group: {relative}; memory.max={memory}; swap.max={swap}',
          file=sys.stderr, flush=True)
    return group


def write_command_receipt(path, receipt):
    """Publish only complete JSON; an interrupted publication is not success."""
    pending = path.with_suffix('.pending')
    with pending.open('w') as stream:
        json.dump(receipt, stream)
        stream.flush()
        os.fsync(stream.fileno())
    pending.replace(path)


def read_command_receipt(path, unit, arguments):
    try:
        receipt = json.loads(path.read_text())
        if not isinstance(receipt, dict):
            return None
        group = receipt.get('cgroup')
        if (receipt.get('unit') != unit or receipt.get('command') != arguments
                or not isinstance(group, str) or not group.startswith('/sys/fs/cgroup/')
                or '..' in Path(group).parts or Path(group).name != unit + '.service'):
            return None
        return receipt
    except (OSError, ValueError):
        return None


def completed_status(wrapper_status, receipt):
    """The transport's exit alone cannot attest completion of the workload."""
    if receipt is None or receipt.get('state') != 'completed':
        return 2
    child_status = receipt.get('exit')
    if type(child_status) is not int or not 0 <= child_status <= 255:
        return 2
    if wrapper_status != child_status:
        return 2
    return child_status


def group_stopped(receipt, stop_status):
    if stop_status == 0:
        return True
    if receipt is None:
        return False
    group = Path(receipt['cgroup'])
    try:
        return 'populated 0' in (group / 'cgroup.events').read_text().splitlines()
    except FileNotFoundError:
        # --collect can remove a completed group before systemctl stop sees it.
        return not group.exists()
    except OSError:
        return False


def run_inside(arguments):
    group = require_limits()
    unit, receipt_path, *command = arguments
    if not command:
        raise RuntimeError('missing command')
    path = Path(receipt_path)
    receipt = {'unit': unit, 'command': command, 'cgroup': str(group), 'state': 'started'}
    write_command_receipt(path, receipt)
    status = subprocess.call(command)
    status = status if status >= 0 else min(255, 128 - status)
    receipt.update(state='completed', exit=status)
    # Cgroup charge includes descendants and can differ from summed process RSS.
    receipt['resource_observation'] = 'command_return_before_service_stop'
    receipt['resources'] = {}
    for name in ('memory.peak', 'memory.events', 'cpu.stat'):
        try:
            receipt['resources'][name] = (group / name).read_text().strip()
        except OSError:
            receipt['resources'][name] = None
    write_command_receipt(path, receipt)
    return status


def main():
    arguments = sys.argv[1:]
    if arguments and arguments[0] == '--inside':
        return run_inside(arguments[1:])
    runtime, delegated, arguments = command_options(arguments)
    if not DATA_DRIVE.is_mount():
        raise RuntimeError(f'data drive is not mounted: {DATA_DRIVE}')
    scratch = STORAGE / 'scratch'
    scratch.mkdir(parents=True, exist_ok=True)
    temporary = Path(tempfile.mkdtemp(prefix='work-', dir=scratch))
    unit = 'fabric-work-' + uuid.uuid4().hex
    receipt_path = temporary / '.resource-command.json'
    command = [
        'systemd-run', '--user', '--wait', '--pipe', '--collect',
        '--unit=' + unit,
        '--property=MemoryAccounting=yes', '--property=MemoryHigh=16G',
        '--property=MemoryMax=20G', '--property=MemorySwapMax=0',
        '--property=OOMPolicy=kill', '--property=KillMode=control-group',
        '--property=RuntimeMaxSec=' + str(runtime) + 's', '--property=TimeoutStopSec=10s',
        '--working-directory=' + os.getcwd(),
    ]
    if delegated:
        command.append('--property=Delegate=yes')
    # Preserve build/tool locations, including explicitly configured toolchains.
    for key in ('PATH', 'CARGO_HOME', 'RUSTUP_HOME', 'CARGO_BUILD_JOBS', 'RUST_TEST_THREADS', 'CARGO_INCREMENTAL', 'CARGO_PROFILE_DEV_DEBUG', 'CARGO_PROFILE_TEST_DEBUG'):
        if key in os.environ:
            command.append('--setenv=' + key + '=' + os.environ[key])
    command += [
        '--setenv=TMPDIR=' + str(temporary),
        '--setenv=TMP=' + str(temporary),
        '--setenv=TEMP=' + str(temporary),
        '--setenv=FABRIC_SCRATCH_ROOT=' + str(scratch),
        '--setenv=CARGO_TARGET_DIR=' + str(STORAGE / 'cargo'),
        '--setenv=FABRIC_RESOURCE_RUNTIME_SECONDS=' + str(runtime),
    ]
    command += [sys.executable, str(Path(__file__).resolve()), '--inside',
                unit, str(receipt_path), *arguments]
    started = time.time()
    status = None
    wrapper_status = None
    try:
        wrapper_status = subprocess.call(command)
    finally:
        # Ensure no descendant can still use these files before deleting them,
        # including on an interrupted caller. Never delete shared build caches.
        stop_error = None
        try:
            stopped = subprocess.run(['systemctl', '--user', 'stop', unit],
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            stop_status = stopped.returncode
        except OSError as error:
            stop_status = None
            stop_error = str(error)
        child = read_command_receipt(receipt_path, unit, arguments)
        safe = group_stopped(child, stop_status)
        status = completed_status(wrapper_status, child) if safe else 2
        if status == 2:
            print('resource group: incomplete or unconfirmed command; retaining evidence',
                  file=sys.stderr)
        retained = None
        if safe and status == 0:
            shutil.rmtree(temporary)
        elif safe:
            evidence = STORAGE / 'evidence'
            evidence.mkdir(parents=True, exist_ok=True)
            retained = evidence / unit
            temporary.rename(retained)
        receipts = RECEIPTS
        receipts.mkdir(parents=True, exist_ok=True)
        (receipts / (unit + '.json')).write_text(json.dumps({
            'unit': unit, 'command': arguments, 'exit': status,
            'wrapper_exit': wrapper_status, 'command_receipt': child,
            'stop_confirmed': safe,
            'stop_error': stop_error,
            'started_unix': started, 'elapsed_s': time.time() - started,
            'data_drive': str(DATA_DRIVE), 'temporary': str(temporary),
            'temporary_removed': not temporary.exists(),
            'retained_failure_evidence': None if retained is None else str(retained),
            'memory_max_bytes': MAX_BYTES, 'swap_max_bytes': 0,
            'runtime_max_seconds': runtime,
            'delegated': delegated,
        }, indent=2) + '\n')
    return status


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, RuntimeError, ValueError) as error:
        print(f'resource group: NOT RUN: {error}', file=sys.stderr)
        sys.exit(2)
