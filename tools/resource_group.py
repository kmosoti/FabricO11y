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


def main():
    arguments = sys.argv[1:]
    if arguments and arguments[0] == '--inside':
        require_limits()
        arguments = arguments[1:]
        if not arguments:
            raise RuntimeError('missing command')
        os.execvp(arguments[0], arguments)
    runtime, delegated, arguments = command_options(arguments)
    if not DATA_DRIVE.is_mount():
        raise RuntimeError(f'data drive is not mounted: {DATA_DRIVE}')
    scratch = STORAGE / 'scratch'
    scratch.mkdir(parents=True, exist_ok=True)
    temporary = Path(tempfile.mkdtemp(prefix='work-', dir=scratch))
    unit = 'fabric-work-' + uuid.uuid4().hex
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
    for key in ('PATH', 'CARGO_HOME', 'RUSTUP_HOME'):
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
    command += [sys.executable, str(Path(__file__).resolve()), '--inside', *arguments]
    started = time.time()
    status = None
    try:
        status = subprocess.call(command)
        return status
    finally:
        # Ensure no descendant can still use these files before deleting them,
        # including on an interrupted caller. Never delete shared build caches.
        stopped = subprocess.run(['systemctl', '--user', 'stop', unit],
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        # A normally returned --wait invocation has already stopped its service.
        safe = status is not None or stopped.returncode == 0
        retained = None
        if safe and status == 0:
            shutil.rmtree(temporary)
        elif safe:
            evidence = STORAGE / 'evidence'
            evidence.mkdir(parents=True, exist_ok=True)
            retained = evidence / unit
            temporary.rename(retained)
        receipts = Path(__file__).resolve().parents[1] / 'target/resource-containment/runs'
        receipts.mkdir(parents=True, exist_ok=True)
        (receipts / (unit + '.json')).write_text(json.dumps({
            'unit': unit, 'command': arguments, 'exit': status,
            'started_unix': started, 'elapsed_s': time.time() - started,
            'data_drive': str(DATA_DRIVE), 'temporary': str(temporary),
            'temporary_removed': not temporary.exists(),
            'retained_failure_evidence': None if retained is None else str(retained),
            'memory_max_bytes': MAX_BYTES, 'swap_max_bytes': 0,
            'runtime_max_seconds': runtime,
            'delegated': delegated,
        }, indent=2) + '\n')


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, RuntimeError, ValueError) as error:
        print(f'resource group: NOT RUN: {error}', file=sys.stderr)
        sys.exit(2)
