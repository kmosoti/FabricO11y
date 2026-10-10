#!/usr/bin/env python3
"""Bounded guest-local resource observations; read failures stay unmeasured."""
import argparse
import json
from pathlib import Path
import time


def sample(service):
    # systemd unit path is transferred from systemctl's actual ControlGroup.
    path = Path('/sys/fs/cgroup') / service.lstrip('/')
    answer = {}
    for key in ('memory.max', 'memory.swap.max', 'memory.events', 'cpu.stat', 'pids.current'):
        try:
            answer[key] = (path / key).read_text().strip()
        except OSError as error:
            answer[key] = {'unmeasured': str(error)}
    rss = 0
    pids = []
    try:
        # Include service descendants; a child cgroup must not disappear from
        # the process RSS population simply because its parent has no PIDs.
        groups = [path, *(p for p in path.rglob('*') if p.is_dir())]
        processes = sorted({pid for group in groups
                            for pid in (group / 'cgroup.procs').read_text().split()})
        for pid in processes:
            status = Path('/proc') / pid / 'status'
            values = dict(line.split(':', 1) for line in status.read_text().splitlines() if ':' in line)
            resident = int(values['VmRSS'].split()[0]) * 1024
            rss += resident
            pids.append({'pid': int(pid), 'name': values['Name'].strip(), 'rss_bytes': resident})
        answer['rss_bytes'] = rss if pids else None
        answer['processes'] = pids
    except (OSError, KeyError, ValueError) as error:
        answer['rss_bytes'] = None
        answer['processes_error'] = str(error)
    return answer


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--service-cgroup', required=True)
    parser.add_argument('--out', required=True, type=Path)
    args = parser.parse_args()
    if '..' in Path(args.service_cgroup).parts or not args.service_cgroup.startswith('/'):
        raise ValueError('absolute actual service cgroup required')
    boot_id = Path('/proc/sys/kernel/random/boot_id').read_text().strip()
    began = time.monotonic_ns()
    with args.out.open('x') as output:
        for tick in range(330):
            due = began + tick * 1_000_000_000
            time.sleep(max(0, (due - time.monotonic_ns()) / 1e9))
            before = time.monotonic_ns()
            unix = time.time_ns()
            observation = sample(args.service_cgroup)
            after = time.monotonic_ns()
            row = {'tick': tick, 'boot_id': boot_id, 'scheduled_ns': due, 'before_ns': before, 'after_ns': after,
                   'unix_ns': unix, 'service_cgroup': args.service_cgroup, 'resource': observation}
            output.write(json.dumps(row, separators=(',', ':')) + '\n')
            output.flush()


if __name__ == '__main__':
    main()
