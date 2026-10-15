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
HIGH_BYTES = 16 * 1024**3


def storage_root(environ=os.environ):
    """FABRIC_STORAGE_ROOT, else the per-user cache directory (XDG_CACHE_HOME or ~/.cache)."""
    cache = environ.get('XDG_CACHE_HOME') or os.path.join(
        environ.get('HOME') or os.path.expanduser('~'), '.cache')
    return Path(environ.get('FABRIC_STORAGE_ROOT') or os.path.join(cache, 'fabrico11y')).resolve()


STORAGE = storage_root()
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


def memory_limits(mem_total=None, ceiling=MAX_BYTES, page=None):
    """(MemoryHigh, MemoryMax): 75% of RAM up to the ceiling, high at 5/6 up to 16 GiB, page aligned."""
    page = page or os.sysconf('SC_PAGE_SIZE')
    cap = min(ceiling, (mem_total or page * os.sysconf('SC_PHYS_PAGES')) * 3 // 4) // page * page
    if cap < 1024**3:
        raise RuntimeError('insufficient host RAM for a bounded group')
    return min(HIGH_BYTES, cap * 5 // 6) // page * page, cap


def filesystem_type(path):
    return subprocess.run(['stat', '-f', '-c', '%T', path], capture_output=True, text=True).stdout.strip()


def require_storage(root=None):
    """Fail closed unless the storage root is a directory we own on a disk, not in RAM."""
    root = Path(root or STORAGE)
    # Judge the filesystem before creating anything, so an unmounted drive's mount point is not made.
    kind = filesystem_type(next(path for path in (root, *root.parents) if path.exists()))
    if kind in ('', 'tmpfs', 'ramfs'):
        raise RuntimeError(f'storage root must not be RAM-backed: {root} ({kind or "unknown filesystem"})')
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    if not root.is_dir() or root.stat().st_uid != os.getuid():
        raise RuntimeError(f'storage root must be a directory owned by the invoking user: {root}')
    return root


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
    observe_final = arguments[:1] == ['--observe-final-group']
    if observe_final:
        arguments = arguments[1:]
    runtime, delegated, arguments = command_options(arguments)
    if observe_final and (runtime != 7500 or not delegated):
        raise ValueError('R3 final observation requires exact 7500 seconds and delegation')
    require_storage()
    high, cap = memory_limits()
    scratch = STORAGE / 'scratch'
    scratch.mkdir(parents=True, exist_ok=True)
    temporary = Path(tempfile.mkdtemp(prefix='work-', dir=scratch))
    unit = 'fabric-work-' + uuid.uuid4().hex
    receipt_path = temporary / '.resource-command.json'
    command = [
        'systemd-run', '--user', '--wait', '--pipe', '--collect',
        '--unit=' + unit,
        '--property=MemoryAccounting=yes', '--property=MemoryHigh=' + str(high),
        '--property=MemoryMax=' + str(cap), '--property=MemorySwapMax=0',
        '--property=OOMPolicy=kill', '--property=KillMode=control-group',
        '--property=RuntimeMaxSec=' + str(runtime) + 's', '--property=TimeoutStopSec=10s',
        '--working-directory=' + os.getcwd(),
    ]
    if delegated:
        command.append('--property=Delegate=yes')
    if observe_final:
        command.append('--property=RemainAfterExit=yes')
    # Preserve build/tool locations, including explicitly configured toolchains.
    for key in ('PATH', 'CARGO_HOME', 'RUSTUP_HOME', 'CARGO_BUILD_JOBS', 'RUST_TEST_THREADS', 'CARGO_INCREMENTAL', 'CARGO_PROFILE_DEV_DEBUG', 'CARGO_PROFILE_TEST_DEBUG'):
        if key in os.environ:
            command.append('--setenv=' + key + '=' + os.environ[key])
    command += [
        '--setenv=TMPDIR=' + str(temporary),
        '--setenv=TMP=' + str(temporary),
        '--setenv=TEMP=' + str(temporary),
        '--setenv=FABRIC_STORAGE_ROOT=' + str(STORAGE),
        '--setenv=FABRIC_SCRATCH_ROOT=' + str(scratch),
        '--setenv=CARGO_TARGET_DIR=' + str(STORAGE / 'cargo'),
        '--setenv=FABRIC_RESOURCE_RUNTIME_SECONDS=' + str(runtime),
    ]
    command += [sys.executable, str(Path(__file__).resolve()), '--inside',
                unit, str(receipt_path), *arguments]
    started = time.time()
    status = None
    wrapper_status = None
    final_observation = None
    try:
        if observe_final:
            from resource_final_group import observe_service
            wrapper_status, final_observation = observe_service(command, receipt_path, unit, arguments,
                                                               runtime, read_command_receipt)
        else:
            wrapper_status = subprocess.call(command)
    finally:
        # Ensure no descendant can still use these files before deleting them,
        # including on an interrupted caller. Never delete shared build caches.
        stop_error = None
        try:
            stopped = subprocess.run(['systemctl', '--user', 'stop', unit],
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                     **({'timeout': 5} if observe_final else {}))
            stop_status = stopped.returncode
        except (OSError, subprocess.TimeoutExpired) as error:
            stop_status = None
            stop_error = str(error)
        child = read_command_receipt(receipt_path, unit, arguments)
        safe = group_stopped(child, stop_status)
        status = completed_status(wrapper_status, child) if safe else 2
        if observe_final and (final_observation is None or final_observation.get('state') != 'observed_empty'):
            status = 2
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
            'storage_root': str(STORAGE), 'temporary': str(temporary),
            'temporary_removed': not temporary.exists(),
            'retained_failure_evidence': None if retained is None else str(retained),
            'memory_high_bytes': high, 'memory_max_bytes': cap, 'swap_max_bytes': 0,
            'runtime_max_seconds': runtime,
            'delegated': delegated,
            **({'final_kernel_observation': final_observation,
                'activation_monotonic_ns': final_observation.get('activation_monotonic_ns')
                    if final_observation else None} if observe_final else {}),
        }, indent=2) + '\n')
    return status


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, RuntimeError, ValueError) as error:
        print(f'resource group: NOT RUN: {error}', file=sys.stderr)
        sys.exit(2)
