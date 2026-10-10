"""Serialized native frontier ledger; run through tools/resource_group.py.

Derived from run_sweep_job.py; historical receipts and frozen runners are untouched.
"""
import argparse
import fcntl
import gzip
import hashlib
import json
import math
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
from resource_group import DATA_DRIVE, STORAGE, require_limits
sys.path.insert(0, str(ROOT / 'tools/bench/labs/catalog'))
import coupled_admit

BASE = ROOT / 'docs/experiments/benchmarks/data/native-frontier-01'
PROTOCOL = ROOT / 'docs/experiments/benchmarks/native-frontier-protocol.md'
HISTORY = {name: ROOT / 'docs/experiments/benchmarks/data' / name / 'coordinator'
           for name in ('lab-completion-run-01', 'cross-system-run-01', 'cross-system-sweep-01')}
MIB = 1024**2
CAPS = {'memory': 512, 'query': 384, 'coordinator': 128}
ROUND_SECONDS = 7200
FRONTIER_SECONDS = 28800
CAMPAIGN_SECONDS = 86400
SCRATCH_BYTES = 8 * 1024**3
FREE_BYTES = 16 * 1024**3
OUTPUT_BYTES = 8 * MIB


def dump(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def footprint(path):
    total = 0
    if path.is_symlink():
        raise RuntimeError('linked evidence/scratch root')
    for p in path.rglob('*'):
        try:
            item = p.lstat()
        except FileNotFoundError:
            continue  # transient cleanup races; final leftover gate remains exact
        if stat.S_ISREG(item.st_mode):
            total += max(item.st_size, item.st_blocks * 512)
        elif not stat.S_ISDIR(item.st_mode):
            raise RuntimeError('linked or special evidence/scratch member')
    return total


def ledger_totals(ledgers):
    totals = {}
    for name, records in ledgers.items():
        elapsed = 0.0
        for record in records:
            if record.get('state') == 'running':
                raise RuntimeError('existing running receipt requires resolution')
            value = record.get('elapsed_s')
            if (record.get('state') not in ('passed', 'failed', 'complete', 'baseline-rejected')
                    or isinstance(value, bool) or not isinstance(value, (int, float))
                    or not math.isfinite(value) or value < 0):
                raise RuntimeError('historical/native ledger missing finite completed duration')
            elapsed += value
        totals[name] = elapsed
    old = totals['lab-completion-run-01']
    prior = totals['cross-system-run-01']
    sweep = totals['cross-system-sweep-01']
    native = totals['native-frontier-01']
    frontier_old = sum(r['elapsed_s'] for r in ledgers['lab-completion-run-01']
                       if r.get('id', '').startswith(('catalog-', 'frontier-')))
    return dict(old_campaign_s=old, old_frontier_s=frontier_old,
                prior_continuation_s=prior, completed_sweep_s=sweep, native_round_prior_s=native,
                remaining_round_s=ROUND_SECONDS-native,
                remaining_frontier_s=FRONTIER_SECONDS-frontier_old-prior-sweep-native,
                remaining_campaign_s=CAMPAIGN_SECONDS-old-prior-sweep-native)


def admit(seconds, lab, reserve_mib, totals, usage, aggregate, free_bytes):
    if (not 10 <= seconds <= 1700 or lab not in CAPS or not 0 <= reserve_mib <= CAPS[lab]):
        raise RuntimeError('invalid bounded native job allocation')
    if seconds > min(totals[k] for k in ('remaining_round_s', 'remaining_frontier_s', 'remaining_campaign_s')):
        raise RuntimeError('finite native/frontier/campaign allocation exhausted')
    if set(usage) != set(CAPS) or any(usage[k] < 0 or usage[k] > CAPS[k]*MIB for k in CAPS):
        raise RuntimeError('existing native evidence exceeds category allocation')
    if usage[lab] + reserve_mib*MIB > CAPS[lab]*MIB:
        raise RuntimeError('projected native lab evidence exceeds allocation')
    if aggregate + reserve_mib*MIB > sum(CAPS.values())*MIB:
        raise RuntimeError('projected native evidence exceeds 1GiB')
    if free_bytes < FREE_BYTES:
        raise RuntimeError('data drive free reserve unavailable')


def load_ledgers():
    ledgers = {}
    manifests = {}
    for name, directory in {**HISTORY, 'native-frontier-01': BASE / 'coordinator'}.items():
        if name in HISTORY and (not directory.is_dir() or directory.is_symlink()):
            raise RuntimeError('historical ledger unavailable; no budget reset permitted')
        paths = sorted(directory.glob('*/receipt.json'))
        if name in HISTORY and not paths:
            raise RuntimeError('historical ledger empty; no budget reset permitted')
        ledgers[name] = [json.loads(p.read_text()) for p in paths]
        manifests[name] = [{'path': str(p), 'sha256': sha(p)} for p in paths]
    return ledgers, manifests


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


def self_test():
    ledgers = {name: [] for name in (*HISTORY, 'native-frontier-01')}
    ledgers['lab-completion-run-01'] = [dict(id='catalog-fixture', state='baseline-rejected', elapsed_s=100)]
    ledgers['cross-system-run-01'] = [dict(state='failed', elapsed_s=20)]
    ledgers['cross-system-sweep-01'] = [dict(state='complete', elapsed_s=30)]
    ledgers['native-frontier-01'] = [dict(state='passed', elapsed_s=40)]
    totals = ledger_totals(ledgers)
    if totals['remaining_frontier_s'] != 28800-190 or totals['remaining_campaign_s'] != 86400-190:
        raise RuntimeError('positive cumulative accounting differs')
    usage = dict.fromkeys(CAPS, 0)
    admit(10, 'memory', 1, totals, usage, 0, FREE_BYTES)
    rejected = []

    def reject(name, action):
        try:
            action()
        except RuntimeError:
            rejected.append(name)
        else:
            raise RuntimeError('native admission defect accepted: ' + name)

    for key in ('remaining_round_s', 'remaining_frontier_s', 'remaining_campaign_s'):
        changed = dict(totals, **{key: 9})
        reject(key, lambda: admit(10, 'memory', 1, changed, usage, 0, FREE_BYTES))
    reject('free_reserve', lambda: admit(10, 'memory', 1, totals, usage, 0, FREE_BYTES-1))
    reject('category_projection', lambda: admit(10, 'memory', 1, totals, dict(usage, memory=512*MIB), 512*MIB, FREE_BYTES))
    reject('aggregate_projection', lambda: admit(10, 'memory', 1, totals, usage, 1024*MIB, FREE_BYTES))
    reject('service_deadline', lambda: admit(1701, 'memory', 1, totals, usage, 0, FREE_BYTES))
    for name, row in [('running', dict(state='running', elapsed_s=1)),
                      ('missing_duration', dict(state='passed')),
                      ('negative_duration', dict(state='passed', elapsed_s=-1)),
                      ('nonfinite_duration', dict(state='passed', elapsed_s=float('nan')))]:
        changed = dict(ledgers, **{'native-frontier-01': [row]})
        reject(name, lambda: ledger_totals(changed))
    return {'positive_cumulative_accounting_and_admission': True, 'rejected': rejected,
            'scope': 'in-memory bounded admission defects; no workload, I/O mutation or process kill'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--self-test', action='store_true')
    parser.add_argument('--id')
    parser.add_argument('--lab', choices=CAPS)
    parser.add_argument('--seconds', type=int)
    parser.add_argument('--reserve-mib', type=int, default=8)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    started = time.monotonic()
    started_unix_ns = time.time_ns()
    require_limits()
    if args.self_test:
        if args.command or args.id or args.lab or args.seconds:
            parser.error('--self-test accepts no workload arguments')
        print(json.dumps(self_test(), indent=2))
        return 0
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command or not args.id or args.lab is None or args.seconds is None:
        parser.error('id, lab, seconds and command required')
    if not args.id.replace('-', '').replace('_', '').isalnum():
        parser.error('invalid job identifier')
    if not DATA_DRIVE.is_mount():
        raise RuntimeError('required data drive is not mounted')
    temporary = Path(os.environ['TMPDIR']).resolve(strict=True)
    temporary.relative_to((STORAGE / 'scratch').resolve(strict=True))
    group = Path('/sys/fs/cgroup') / next(s[3:] for s in Path('/proc/self/cgroup').read_text().splitlines()
        if s.startswith('0::')).lstrip('/')
    limits = {name: (group / name).read_text().strip() for name in
              ('memory.max', 'memory.high', 'memory.swap.max', 'cpu.max')}
    if (limits['memory.max'] != str(20*1024**3) or limits['memory.high'] != str(16*1024**3)
            or limits['memory.swap.max'] != '0'
            or not 0 < int(os.environ['FABRIC_RESOURCE_RUNTIME_SECONDS']) <= 1800):
        raise RuntimeError('native round requires unchanged resource service limits')
    coord = BASE / 'coordinator'
    coord.mkdir(parents=True, exist_ok=True)
    with (ROOT / 'target/readiness-lab.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        ledgers, ledger_manifest = load_ledgers()
        totals = ledger_totals(ledgers)
        usage = {lab: footprint(BASE / lab) for lab in CAPS}
        admit(args.seconds, args.lab, args.reserve_mib, totals, usage,
              footprint(BASE), shutil.disk_usage(temporary).free)
        before = coupled_admit.observe(0, 0)
        protocol_bytes = PROTOCOL.read_bytes()
        runner_bytes = Path(__file__).read_bytes()
        out = coord / args.id
        out.mkdir()
        work = temporary / args.id
        work.mkdir()
        record = dict(id=args.id, lab=args.lab, state='running', argv=command, timeout_s=args.seconds,
                      **totals, started_unix_ns=started_unix_ns, scratch=str(work), before=before,
                      evidence_before=usage, protocol_sha256=hashlib.sha256(protocol_bytes).hexdigest(),
                      runner_sha256=hashlib.sha256(runner_bytes).hexdigest(), category_caps_mib=CAPS,
                      limits=limits, ledger_manifest=ledger_manifest)
        (out / 'runner.py.gz').write_bytes(gzip.compress(runner_bytes, mtime=0))
        (out / 'protocol.txt.gz').write_bytes(gzip.compress(protocol_bytes, mtime=0))
        (out / 'historical-ledgers.json.gz').write_bytes(gzip.compress(json.dumps(ledgers).encode(), mtime=0))
        dump(out / 'receipt.json', record)
        code = 1
        child = None
        try:
            with (out / 'stdout.txt').open('wb') as stdout, (out / 'stderr.txt').open('wb') as stderr, \
                    gzip.open(out / 'resources.jsonl.gz', 'wt') as samples:
                child = subprocess.Popen(command, cwd=ROOT, stdout=stdout, stderr=stderr,
                    env=dict(os.environ, FABRIC_SCRATCH_ROOT=str(work),
                             FABRIC_CROSS_SYSTEM_COORDINATED='1', FABRIC_NATIVE_COORDINATED='1'),
                    start_new_session=True)
                while child.poll() is None:
                    elapsed = time.monotonic() - started
                    samples.write(json.dumps(dict(elapsed_s=elapsed, **{n: (group / n).read_text().strip()
                        for n in ('memory.current', 'memory.stat', 'memory.events', 'memory.pressure', 'cpu.stat', 'io.stat')})) + '\n')
                    samples.flush()
                    if elapsed > args.seconds - 5:
                        raise TimeoutError('registered native job deadline')
                    if stdout.tell() + stderr.tell() > OUTPUT_BYTES:
                        raise RuntimeError('command output exceeds 8MiB')
                    if footprint(work) > SCRATCH_BYTES or shutil.disk_usage(work).free < FREE_BYTES:
                        raise RuntimeError('scratch or free reserve exceeded')
                    time.sleep(.5)
                code = child.returncode
                if code:
                    raise RuntimeError(f'child exit {code}')
                if time.monotonic() - started > args.seconds - 5:
                    raise TimeoutError('finished child exceeded registered native deadline')
                if stdout.tell() + stderr.tell() > OUTPUT_BYTES:
                    raise RuntimeError('finished child output exceeds 8MiB')
                if footprint(work) > SCRATCH_BYTES or shutil.disk_usage(work).free < FREE_BYTES:
                    raise RuntimeError('finished child scratch/free reserve exceeded')
            stop_descendants(group)
            if any(work.iterdir()):
                raise RuntimeError('command left owned scratch; preserve before cleanup')
            work.rmdir()
            record['state'] = 'passed'
        except BaseException as error:
            record.update(state='failed', error=repr(error))
            code = code or 1
            try:
                stop_descendants(group)
                if child is not None:
                    child.wait(timeout=2)
            except BaseException as shutdown_error:
                record['shutdown_error'] = repr(shutdown_error)
        finally:
            record['scratch_removed'] = not work.exists()
            record['exit'] = code
            record['child_exit'] = None if child is None else child.poll()
            record['cgroup_final'] = {name: (group / name).read_text().strip() for name in
                ('memory.peak', 'memory.events', 'memory.swap.current', 'cpu.stat', 'io.stat')}
            try:
                record['evidence_after'] = {lab: footprint(BASE / lab) for lab in CAPS}
                record['aggregate_evidence_after'] = footprint(BASE)
                overage = (any(record['evidence_after'][lab] > cap*MIB for lab, cap in CAPS.items())
                           or record['aggregate_evidence_after'] > sum(CAPS.values())*MIB)
            except BaseException as census_error:
                record['evidence_census_error'] = repr(census_error)
                overage = True
            if overage:
                code = 1
                record.update(state='failed', exit=1, evidence_overage=True)
            record['elapsed_s'] = time.monotonic() - started
            record['finished_unix_ns'] = time.time_ns()
            dump(out / 'receipt.json', record)
        return code


if __name__ == '__main__':
    raise SystemExit(main())
