#!/usr/bin/python3
"""A12-only bounded positive control and concurrent anonymous-page stressor."""
import argparse
import mmap
import os
import signal
import time
from pathlib import Path

CONTROL_BYTES = 32 * 1024**2
MAX_BYTES = 128 * 1024**2
WORKERS = 96


def effective_limits(group: Path, root: Path) -> dict[str, int | None]:
    effective = {'memory.high': None, 'memory.max': None, 'memory.swap.max': None,
                 'pids.max': None}
    path = group
    while path != root and root in path.parents:
        for name in effective:
            limit_file = path / name
            if not limit_file.is_file():
                continue
            raw = limit_file.read_text().strip()
            value = None if raw == 'max' else int(raw)
            if value is not None:
                effective[name] = value if effective[name] is None else min(effective[name], value)
        path = path.parent
    return effective


def verify_cgroup(memory_high: int, memory_max: int, tasks_max: int) -> None:
    relative = next(line[3:] for line in Path('/proc/self/cgroup').read_text().splitlines()
                    if line.startswith('0::'))
    root = Path('/sys/fs/cgroup')
    group = root / relative.lstrip('/')
    effective = effective_limits(group, root)
    expected = {'memory.high': memory_high, 'memory.max': memory_max,
                'memory.swap.max': 0, 'pids.max': tasks_max}
    if effective != expected:
        raise RuntimeError(f'fixture effective cgroup limits mismatch: {effective!r}')


def positive_control() -> None:
    pagesize = os.sysconf('SC_PAGE_SIZE')
    if CONTROL_BYTES % pagesize:
        raise RuntimeError('control allocation is not page aligned')
    allocation = mmap.mmap(
        -1, CONTROL_BYTES,
        flags=mmap.MAP_PRIVATE | mmap.MAP_ANONYMOUS | mmap.MAP_POPULATE,
        prot=mmap.PROT_READ | mmap.PROT_WRITE,
    )
    # Keep the populated pages resident long enough for the acceptance sampler.
    time.sleep(3)
    allocation.close()


def parallel_pressure(size: int) -> None:
    pagesize = os.sysconf('SC_PAGE_SIZE')
    if size <= 0 or size > MAX_BYTES or size % pagesize:
        raise ValueError(f'worker bytes must be page-aligned and <= {MAX_BYTES}')

    start_read, start_write = os.pipe()
    children = []
    for _ in range(WORKERS):
        pid = os.fork()
        if pid == 0:
            os.close(start_write)
            if os.read(start_read, 1) != b'x':
                os._exit(80)
            os.close(start_read)
            allocation = mmap.mmap(
                -1, size,
                flags=mmap.MAP_PRIVATE | mmap.MAP_ANONYMOUS,
                prot=mmap.PROT_READ | mmap.PROT_WRITE,
            )
            for offset in range(0, size, pagesize):
                allocation[offset] = 1
            # The systemd unit's hard memory cap must end this fixture.
            while True:
                signal.pause()
        children.append(pid)

    os.close(start_read)
    os.write(start_write, b'x' * WORKERS)
    os.close(start_write)
    # Keep the unit's main process alive with the workers. OOMPolicy=kill makes
    # the kernel terminate the whole transient unit after its first OOM victim.
    while True:
        time.sleep(60)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--control', action='store_true')
    mode.add_argument('--parallel-bytes', type=int)
    parser.add_argument('--memory-high-bytes', type=int)
    parser.add_argument('--memory-max-bytes', type=int)
    parser.add_argument('--tasks-max', type=int)
    args = parser.parse_args()
    if args.control:
        if (args.memory_high_bytes, args.memory_max_bytes, args.tasks_max) != (
                128 * 1024**2, 256 * 1024**2, 128):
            parser.error('--control requires the node MemoryHigh/MemoryMax/TasksMax values')
        verify_cgroup(args.memory_high_bytes, args.memory_max_bytes, args.tasks_max)
        positive_control()
    else:
        if not all(value is not None for value in (
                args.memory_high_bytes, args.memory_max_bytes, args.tasks_max)):
            parser.error('--parallel-bytes requires expected cgroup limits')
        verify_cgroup(args.memory_high_bytes, args.memory_max_bytes, args.tasks_max)
        parallel_pressure(args.parallel_bytes)


if __name__ == '__main__':
    main()
