"""Finite matched-workload sweep ledger; invoke inside the existing resource launcher."""
import argparse
import fcntl
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import stat
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits
sys.path.insert(0, str(ROOT / 'tools/bench/labs/catalog'))
import coupled_admit

BASE = ROOT / 'docs/experiments/benchmarks/data/cross-system-sweep-01'
OLD = ROOT / 'docs/experiments/benchmarks/data/lab-completion-run-01/coordinator'
PROTOCOL = ROOT / 'docs/experiments/benchmarks/cross-system-sweep-protocol.md'
ALLOCATION = ROOT / 'docs/experiments/benchmarks/cross-system-evidence-allocation-supplement.md'
MIB = 1024**2
CAPS = {'source': 768, 'memory': 256, 'query': 512, 'operations': 384, 'coordinator': 128}


def dump(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def footprint(path):
    total = 0
    for p in path.rglob('*'):
        try:
            s = p.lstat()
        except FileNotFoundError:
            continue
        if stat.S_ISLNK(s.st_mode):
            raise RuntimeError('linked continuation evidence/scratch')
        if stat.S_ISREG(s.st_mode):
            total += max(s.st_size, s.st_blocks * 512)
    return total


def stop_descendants(group):
    for _ in range(100):
        pids = {int(p) for f in [group / 'cgroup.procs', *group.glob('**/cgroup.procs')]
                for p in f.read_text().split()} - {os.getpid()}
        if not pids:
            return
        for pid in pids:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        time.sleep(.02)
    raise RuntimeError('descendants remain in owned cgroup')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--id', required=True)
    parser.add_argument('--lab', choices=CAPS, required=True)
    parser.add_argument('--seconds', type=int, required=True)
    parser.add_argument('--reserve-mib', type=int, default=8)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command or not 1 <= args.seconds <= 1700 or not 0 <= args.reserve_mib <= CAPS[args.lab]:
        parser.error('command or finite allocation invalid')
    if not args.id.replace('-', '').replace('_', '').isalnum():
        parser.error('invalid job identifier')
    started = time.monotonic()
    require_limits()
    coord = BASE / 'coordinator'
    coord.mkdir(parents=True, exist_ok=True)
    with (ROOT / 'target/readiness-lab.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        old = [json.loads(p.read_text()) for p in OLD.glob('*/receipt.json')]
        new = [json.loads(p.read_text()) for p in coord.glob('*/receipt.json')]
        prior = [json.loads(p.read_text()) for p in (ROOT / 'docs/experiments/benchmarks/data/cross-system-run-01/coordinator').glob('*/receipt.json')]
        prior_s = sum(r.get('elapsed_s', 0) for r in prior)
        if any(r['state'] == 'running' for r in old + prior + new):
            raise RuntimeError('existing running receipt requires resolution')
        old_s = sum(r.get('elapsed_s', 0) for r in old)
        frontier_s = sum(r.get('elapsed_s', 0) for r in old if r['id'].startswith(('catalog-', 'frontier-')))
        new_s = sum(r.get('elapsed_s', 0) for r in new)
        if args.seconds > min(7200 - new_s, 28800 - frontier_s - prior_s - new_s, 86400 - old_s - prior_s - new_s):
            raise RuntimeError('finite continuation/campaign allocation exhausted')
        before = coupled_admit.observe(0, 0)
        usage = {lab: footprint(BASE / lab) for lab in CAPS}
        if any(usage[lab] > cap * MIB for lab, cap in CAPS.items()):
            raise RuntimeError('existing continuation evidence over allocation')
        if usage[args.lab] + args.reserve_mib * MIB > CAPS[args.lab] * MIB:
            raise RuntimeError('projected lab evidence exceeds allocation')
        if footprint(BASE) + args.reserve_mib * MIB > 2048 * MIB:
            raise RuntimeError('projected continuation evidence exceeds 2GiB')
        if shutil.disk_usage(os.environ['TMPDIR']).free < 16 * 1024**3:
            raise RuntimeError('data drive reserve unavailable')
        out = coord / args.id
        out.mkdir()
        work = Path(os.environ['TMPDIR']) / args.id
        work.mkdir()
        group = Path('/sys/fs/cgroup') / next(s[3:] for s in Path('/proc/self/cgroup').read_text().splitlines()
            if s.startswith('0::')).lstrip('/')
        record = dict(id=args.id, lab=args.lab, state='running', argv=command, timeout_s=args.seconds,
            old_campaign_s=old_s, prior_continuation_s=prior_s, old_frontier_s=frontier_s, continuation_prior_s=new_s,
            started_unix_ns=time.time_ns(), scratch=str(work), before=before,
            evidence_before=usage, protocol_sha256=sha(PROTOCOL), runner_sha256=sha(Path(__file__)),
            allocation_supplement_sha256=sha(ALLOCATION), category_caps_mib=CAPS,
            limits={name: (group / name).read_text().strip() for name in
                    ('memory.max', 'memory.high', 'memory.swap.max', 'cpu.max')})
        (out / 'runner.py.gz').write_bytes(gzip.compress(Path(__file__).read_bytes(), mtime=0))
        (out / 'protocol.txt.gz').write_bytes(gzip.compress(PROTOCOL.read_bytes(), mtime=0))
        (out / 'allocation-supplement.txt.gz').write_bytes(gzip.compress(ALLOCATION.read_bytes(), mtime=0))
        dump(out / 'receipt.json', record)
        code = 1
        child = None
        try:
            with (out / 'stdout.txt').open('wb') as stdout, (out / 'stderr.txt').open('wb') as stderr, \
                    gzip.open(out / 'resources.jsonl.gz', 'wt') as samples:
                child = subprocess.Popen(command, cwd=ROOT, stdout=stdout, stderr=stderr,
                    env=dict(os.environ, FABRIC_SCRATCH_ROOT=str(work),
                             FABRIC_CROSS_SYSTEM_COORDINATED='1'), start_new_session=True)
                while child.poll() is None:
                    elapsed = time.monotonic() - started
                    samples.write(json.dumps(dict(elapsed_s=elapsed, **{n: (group / n).read_text().strip()
                        for n in ('memory.current', 'memory.stat', 'memory.events', 'memory.pressure', 'cpu.stat', 'io.stat')})) + '\n')
                    samples.flush()
                    if elapsed > args.seconds - 5:
                        raise TimeoutError('registered job deadline')
                    if stdout.tell() + stderr.tell() > 8 * MIB:
                        raise RuntimeError('command output exceeds 8MiB')
                    if footprint(work) > 8 * 1024**3 or shutil.disk_usage(work).free < 16 * 1024**3:
                        raise RuntimeError('scratch or free reserve exceeded')
                    time.sleep(.5)
                code = child.returncode
                if code:
                    raise RuntimeError(f'child exit {code}')
            stop_descendants(group)
            if any(work.iterdir()):
                raise RuntimeError('command left owned scratch; preserve before cleanup')
            work.rmdir()
            record['state'] = 'passed'
        except BaseException as error:
            record.update(state='failed', error=repr(error))
            code = code or 1
            stop_descendants(group)
            if child is not None:
                child.wait()
        finally:
            record['scratch_removed'] = not work.exists()
            record['exit'] = code
            record['child_exit'] = None if child is None else child.returncode
            record['cgroup_final'] = {name: (group / name).read_text().strip() for name in
                ('memory.peak', 'memory.events', 'memory.swap.current', 'cpu.stat', 'io.stat')}
            try:
                record['evidence_after'] = {lab: footprint(BASE / lab) for lab in CAPS}
                overage = any(record['evidence_after'][lab] > cap * MIB for lab, cap in CAPS.items())
            except BaseException as census_error:
                record['evidence_census_error'] = repr(census_error)
                overage = True
            if overage:
                code = 1
                record.update(state='failed', exit=1, evidence_overage=True)
            record['elapsed_s'] = time.monotonic() - started
            dump(out / 'receipt.json', record)
        return code


if __name__ == '__main__':
    raise SystemExit(main())
