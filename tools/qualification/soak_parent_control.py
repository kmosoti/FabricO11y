"""Explicit finite native controls for the unqualified R3 launcher mode."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from resource_group import require_limits, STORAGE
from resource_final_group import FIELDS


def require(value, message):
    if not value:
        raise ValueError(message)


def identity(pid):
    process = Path('/proc') / str(pid)
    fields = (process / 'stat').read_text().rsplit(')', 1)[1].split()
    return {'pid': pid, 'start_ticks': int(fields[19]),
            'boot_id': Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
            'cgroup': (process / 'cgroup').read_text()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--case', choices=('positive', 'orphan', 'early', 'missing'), required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    signal.signal(signal.SIGALRM, lambda *_: (_ for _ in ()).throw(TimeoutError('finite native control60s')))
    signal.alarm(60)
    require(os.environ.get('FABRIC_RESOURCE_RUNTIME_SECONDS') == '7500', 'exact R3 launcher required')
    group = require_limits()
    require(group.name.startswith('fabric-work-') and group.name.endswith('.service'), 'owned service required')
    fields = {key: (group / key).read_text().strip() for key in FIELDS}
    require(all(fields[key] == value for key, value in {'memory.high': '5368709120',
        'memory.max': '6442450944', 'memory.swap.max': '0', 'pids.max': '1536'}.items()),
        'native control actual cap mismatch')
    require(args.out.is_absolute() and not args.out.exists() and not args.out.is_symlink(), 'fresh native output required')
    args.out.parent.resolve(strict=True).relative_to(STORAGE / 'results')
    args.out.mkdir(mode=0o700)
    supervisor = group / 'supervisor'
    supervisor.mkdir()
    for pid in (group / 'cgroup.procs').read_text().split():
        (supervisor / 'cgroup.procs').write_text(pid)
    (group / 'cgroup.subtree_control').write_text('+memory +cpu +pids +io')
    for key, value in {'memory.max': str(2 * 1024**3), 'memory.high': str(1700 * 1024**2),
                       'memory.swap.max': '0', 'pids.max': '512', 'cpu.max': '400000 100000'}.items():
        (supervisor / key).write_text(value)
        require((supervisor / key).read_text().strip() == value, 'native supervisor cap mismatch')
    record = {'scope': 'unqualified launcher native control only; not S1/FULL', 'case': args.case,
        'at_ns': time.monotonic_ns(), 'process': identity(os.getpid()), 'parent': str(group),
        'parent_identity': {'device': group.stat().st_dev, 'inode': group.stat().st_ino},
        'admission_fields': fields, 'source_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    if args.case == 'orphan':
        child = subprocess.Popen([sys.executable, '-B', '-c', 'import time; time.sleep(120)'])
        record['owned_idle_child'] = identity(child.pid)
        require(child.poll() is None, 'idle child exited before actual live observation')
    if args.case == 'missing':
        parent = os.getppid()
        record['inside_wrapper'] = identity(parent)
        cmdline = (Path('/proc') / str(parent) / 'cmdline').read_bytes().split(b'\0')
        require(parent > 1 and b'--inside' in cmdline and
                any(part.endswith(b'/resource_group.py') for part in cmdline)
                and record['inside_wrapper']['cgroup'] == record['process']['cgroup'],
                'refuse to signal unrelated process')
    with (args.out / 'native.json').open('x') as stream:
        json.dump(record, stream, sort_keys=True)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())
    if args.case == 'missing':
        require(identity(parent) == record['inside_wrapper'], 'inside wrapper identity changed')
        os.kill(parent, signal.SIGKILL)
    return 7 if args.case == 'early' else 0


if __name__ == '__main__':
    raise SystemExit(main())
