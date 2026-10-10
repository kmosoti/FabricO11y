#!/usr/bin/env python3
"""Coordinator-only, serialized job execution inside the resource launcher."""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tarfile
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT/'tools'))
from resource_group import require_limits, STORAGE

DATA = ROOT/'docs/experiments/benchmarks/data/readiness-labs-run-01/coordinator'


def write(path, value):
    temporary = path.with_suffix('.writing')
    temporary.write_text(json.dumps(value, indent=2)+'\n')
    temporary.replace(path)


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def footprint(path):
    total = 0
    for p in path.rglob('*'):
        try:
            if p.is_file() and not p.is_symlink():
                total += p.stat().st_size
        except FileNotFoundError:
            pass  # Live rename/reclaim is expected.
    return total


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--id', required=True)
    parser.add_argument('--lab', choices=['memory','query','recovery','coordinator'], required=True)
    parser.add_argument('--seconds', type=int, default=900)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    require_limits()
    if not re.fullmatch('[a-z0-9][a-z0-9_-]{0,63}', args.id) or not 0 < args.seconds <= 1700:
        parser.error('invalid job ID or deadline; leave time inside 30-minute launcher for cleanup')
    command = args.command[1:] if args.command[:1]==['--'] else args.command
    if not command:
        parser.error('missing command')
    (ROOT/'target').mkdir(exist_ok=True)
    with (ROOT/'target/readiness-lab.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        completed = [json.loads(p.read_text()) for p in DATA.glob('*/receipt.json')]
        consumed = sum(r.get('elapsed_s', 0) for r in completed)
        available = int(3600 - consumed)
        if available <= 0:
            raise RuntimeError('campaign execution budget of 60 minutes exhausted')
        requested_seconds = args.seconds
        args.seconds = min(args.seconds, available)
        destination = DATA/args.id
        destination.mkdir(parents=True, exist_ok=False)
        temporary = Path(os.environ['TMPDIR']).resolve()
        if not temporary.is_relative_to(STORAGE/'scratch'):
            raise RuntimeError('launcher-owned data-drive scratch required')
        work = temporary/args.lab/args.id
        work.mkdir(parents=True, exist_ok=False)
        (work/'tmp').mkdir()
        (work/'scratch').mkdir()
        environment = dict(os.environ, FABRIC_LAB_SCRATCH=str(work), TMPDIR=str(work/'tmp'),
                           FABRIC_SCRATCH_ROOT=str(work/'scratch'),
                           TMP=str(work/'tmp'), TEMP=str(work/'tmp'), PYTHONDONTWRITEBYTECODE='1')
        source = subprocess.check_output(['git','ls-files','-z','--cached','--others','--exclude-standard'],cwd=ROOT).decode().split('\0')
        with tarfile.open(destination/'source.tar.gz', 'x:gz') as archive:
            for name in source:
                p = ROOT/name
                if name and p.is_file() and not name.startswith('docs/experiments/benchmarks/data/') and (p.suffix in ('.rs','.py') or p.name in ('Cargo.toml','Cargo.lock')):
                    archive.add(p, arcname=name)
        (destination/'working-tree.diff').write_bytes(subprocess.check_output(['git','diff','HEAD'],cwd=ROOT))
        protocols = [ROOT/'docs/experiments/benchmarks/dev-small-readiness-plan.md']
        protocols += list((ROOT/'tools/bench/labs/readiness'/args.lab).glob('*.md'))
        protocols += list((DATA.parent/args.lab).glob('*.md'))
        hashes = {}
        for n,p in enumerate(protocols):
            hashes[str(p.relative_to(ROOT))] = digest(p)
            shutil.copy(p, destination/f'protocol-{n}.txt')
        relative = next(s[3:] for s in Path('/proc/self/cgroup').read_text().splitlines() if s.startswith('0::'))
        group = Path('/sys/fs/cgroup')/relative.lstrip('/')
        record = {'id':args.id, 'lab':args.lab, 'state':'running', 'command':command,
                  'cwd':str(ROOT), 'scratch':str(work), 'started_unix_ns':time.time_ns(),
                  'timeout_s':args.seconds, 'scratch_limit_bytes':4*1024**3,
                  'requested_timeout_s':requested_seconds, 'prior_execution_s':consumed,
                  'cgroup':relative, 'protocol_hashes':hashes,
                  'revision':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
                  'limits':{k:(group/k).read_text().strip() for k in ('memory.max','memory.high','memory.swap.max','cpu.max')}}
        write(destination/'receipt.json',record)
        started = time.monotonic()
        peak_disk, reason, child = 0, None, None
        try:
            with (destination/'stdout.txt').open('wb') as out, (destination/'stderr.txt').open('wb') as err:
                child = subprocess.Popen(command,cwd=ROOT,env=environment,stdout=out,stderr=err,start_new_session=True)
                while child.poll() is None:
                    peak_disk = max(peak_disk,footprint(temporary))
                    if time.monotonic()-started > args.seconds:
                        reason = 'timeout'
                    elif peak_disk > 4*1024**3 or shutil.disk_usage(STORAGE).free < 4*1024**3:
                        reason = 'scratch/free-space limit'
                    if reason:
                        os.killpg(child.pid,signal.SIGKILL)
                        child.wait(timeout=10)
                        break
                    time.sleep(.5)
            record.update(exit=child.returncode, state='passed' if child.returncode==0 and reason is None else 'failed')
        except BaseException as exc:
            record.update(state='interrupted', error=repr(exc))
            raise
        finally:
            if child is not None:
                # Stop any descendant which outlived its parent before cleanup.
                try:
                    os.killpg(child.pid,signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait(timeout=10)
            # Fixtures may start their own sessions. The service cgroup is owned
            # by this one serialized job, so stop those descendants as well.
            survivors = [int(p) for p in (group/'cgroup.procs').read_text().split() if int(p)!=os.getpid()]
            for pid in survivors:
                try:
                    os.kill(pid,signal.SIGKILL)
                except ProcessLookupError:
                    pass
            for _ in range(50):
                remaining = [p for p in (group/'cgroup.procs').read_text().split() if int(p)!=os.getpid()]
                if not remaining:
                    break
                time.sleep(.1)
            if remaining:
                record.update(state='failed', error='descendants remained at cleanup')
            record.update(elapsed_s=time.monotonic()-started, stop_reason=reason,
                          peak_sampled_scratch_bytes=peak_disk,
                          terminated_descendants=survivors,
                          cgroup_final={k:(group/k).read_text().strip() for k in
                                        ('memory.peak','memory.current','memory.stat','memory.events','memory.swap.current','cpu.stat','io.stat','memory.pressure','io.pressure')})
            if record['state']=='passed':
                shutil.rmtree(work)
            record['scratch_removed'] = not work.exists()
            record['failed_scratch_retained_by_launcher'] = record['state']!='passed'
            write(destination/'receipt.json',record)
            receipts = [json.loads(p.read_text()) for p in DATA.glob('*/receipt.json')]
            write(DATA.parent/'queue.json', {
                'scope':'Command outcomes; scientific acceptance is recorded separately in the run record.',
                'completed_execution_seconds':sum(r.get('elapsed_s',0) for r in receipts),
                'jobs':[{k:r.get(k) for k in ('id','lab','state','exit','started_unix_ns','elapsed_s','scratch_removed')}
                        for r in sorted(receipts, key=lambda r:r['started_unix_ns'])]})
        print(json.dumps(record),flush=True)
        raise SystemExit(0 if record['state']=='passed' else 1)


if __name__=='__main__':
    main()
