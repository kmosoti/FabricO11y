#!/usr/bin/env python3
"""Prospective path-preserving catalog evidence reclamation, never native work."""
import argparse
import collections
import contextlib
import fcntl
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time

import coupled_admit
from coupled_cleanup import inactive
from resource_group import STORAGE, require_limits

ROOT = Path(__file__).resolve().parents[4]
BASE = ROOT / 'docs/experiments/benchmarks/data'
COORD = BASE / 'lab-completion-run-01/coordinator'
CAPACITY = ('catalog-borrowed-log-run-01', 'catalog-many-segment-run-01',
            'catalog-many-segment-run-02')
CHUNK = 1024 * 1024
MAX_PAYLOAD = 1024**3


def regular(path):
    """Reject indirect ownership through every parent below the repository root."""
    path = Path(path)
    for parent in (path, *path.parents):
        if parent == ROOT:
            break
        if parent.is_symlink():
            raise RuntimeError('unexpected linked evidence: ' + str(parent))
    if not stat.S_ISREG(path.lstat().st_mode):
        raise RuntimeError('nonregular evidence: ' + str(path))
    return path.stat()


def digest(path, decoded=False):
    regular(path)
    size, h = 0, hashlib.sha256()
    with (gzip.open if decoded else open)(path, 'rb') as stream:
        while block := stream.read(CHUNK):
            size += len(block)
            if size > MAX_PAYLOAD:
                raise RuntimeError('payload exceeds one GiB bound')
            h.update(block)
    return h.hexdigest(), size


def equal(left, right, decoded=False):
    regular(left)
    regular(right)
    with (gzip.open if decoded else open)(left, 'rb') as a, (gzip.open if decoded else open)(right, 'rb') as b:
        size = 0
        while True:
            x, y = a.read(CHUNK), b.read(CHUNK)
            size += len(x)
            if size > MAX_PAYLOAD or x != y:
                raise RuntimeError('exact payload comparison rejected')
            if not x:
                return


def write(path, value):
    temporary = path.with_suffix(path.suffix + '.writing')
    with temporary.open('w') as stream:
        stream.write(json.dumps(value, indent=2) + '\n')
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)


def controls(work):
    good, changed, truncated = [work / name for name in ('good.gz', 'changed.gz', 'truncated.gz')]
    for path, payload in ((good, b'actual bytes\n'), (changed, b'changed bytes\n')):
        with gzip.open(path, 'wb') as stream:
            stream.write(payload)
    truncated.write_bytes(good.read_bytes()[:-5])
    link = work / 'unexpected.gz'
    link.symlink_to(good)
    equal(good, good, True)
    rejected = []
    for name, bad in (('changed_payload', changed), ('truncated_gzip', truncated),
                      ('missing_canonical', work / 'missing.gz'), ('unexpected_link', link)):
        try:
            equal(good, bad, True)
        except (RuntimeError, OSError, EOFError):
            rejected.append(name)
        else:
            raise RuntimeError('negative control accepted: ' + name)
    return rejected


def terminal(receipt):
    regular(receipt)
    row = json.loads(receipt.read_text())
    if row.get('id') != receipt.parent.name or row.get('state') not in ('passed', 'failed', 'interrupted'):
        raise RuntimeError('job not terminal or identity mismatch: ' + str(receipt))
    unit = Path(row.get('cgroup', '')).name.removesuffix('.service')
    if not re.fullmatch(r'fabric-work-[0-9a-f]{32}', unit):
        raise RuntimeError('unknown owned unit: ' + str(receipt))
    inactive(unit)
    return row


def main():
    started = time.monotonic()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--id', default='catalog-coupled-reclaim-01')
    parser.add_argument('--protocol', type=Path, default=ROOT / 'docs/experiments/benchmarks/coupled-reclamation-protocol.md')
    parser.add_argument('--seconds', type=int, default=180)
    args = parser.parse_args()
    if not re.fullmatch(r'catalog-coupled-reclaim-[0-9]{2}', args.id) or not 0 < args.seconds <= 180:
        parser.error('fresh scoped ID and at most180seconds required')
    require_limits()
    protocol = args.protocol.resolve(strict=True)
    if protocol.parent != ROOT / 'docs/experiments/benchmarks' or protocol.name not in ('coupled-reclamation-protocol.md', 'coupled-reclamation-retry-protocol.md'):
        raise RuntimeError('exact registered coupled-reclaim protocol required')
    regular(args.protocol)
    regular(Path(__file__))
    scratch_parent = Path(os.environ['TMPDIR']).resolve(strict=True)
    if not scratch_parent.is_relative_to(STORAGE / 'scratch') or not STORAGE.parent.is_mount():
        raise RuntimeError('owned mounted data-drive scratch required')
    if shutil.disk_usage(STORAGE).free < 16 * 1024**3:
        raise RuntimeError('less than16GiB free drive reserve')
    (ROOT / 'target').mkdir(exist_ok=True)
    with (ROOT / 'target/readiness-lab.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        receipts = [json.loads(p.read_text()) for p in COORD.glob('*/receipt.json')]
        if any(r.get('state') == 'running' for r in receipts):
            raise RuntimeError('another coordinator job still marked running')
        consumed = sum(r.get('elapsed_s', 0) for r in receipts)
        frontier = sum(r.get('elapsed_s', 0) for r in receipts if r['id'].startswith(('catalog-', 'frontier-')))
        preparation = sum(r.get('elapsed_s', 0) for r in receipts if r.get('stage') == 'preparation')
        if min(86400 - consumed, 14400 - frontier, 3600 - preparation) < args.seconds:
            raise RuntimeError('full deadline does not fit campaign/frontier/preparation budget')
        if int(os.environ.get('FABRIC_RESOURCE_RUNTIME_SECONDS', '0')) < args.seconds + 30:
            raise RuntimeError('outer deadline must leave30seconds cleanup margin')
        destination = COORD / args.id
        if destination.exists():
            raise RuntimeError('fresh bootstrap coordinator ID required')
        # Small provenance, not the normal1.4MiB source snapshot. Exact helper,
        # protocol and hashes survive independently of subsequent HEAD changes.
        before = coupled_admit.observe(128 * 1024, 0)
        destination.mkdir()
        for source in (Path(__file__).resolve(), protocol):
            shutil.copyfile(source, destination / source.name)
            equal(source, destination / source.name)
        relative = next(line[3:] for line in Path('/proc/self/cgroup').read_text().splitlines() if line.startswith('0::'))
        group = Path('/sys/fs/cgroup') / relative.lstrip('/')
        source_paths = (Path(__file__).resolve(), protocol,
                        ROOT / 'tools/bench/labs/catalog/coupled_admit.py',
                        ROOT / 'tools/bench/labs/catalog/campaign_summary.py',
                        ROOT / 'tools/bench/labs/catalog/coupled_resource_readiness.py',
                        ROOT / 'tools/bench/labs/catalog/coupled_cleanup.py',
                        ROOT / 'tools/resource_group.py')
        record = {'id': args.id, 'lab': 'coordinator', 'stage': 'preparation', 'state': 'running',
                  'command': sys.argv, 'cwd': str(ROOT), 'started_unix_ns': time.time_ns(),
                  'timeout_s': args.seconds, 'prior_execution_s': consumed,
                  'frontier_prior_execution_s': frontier, 'frontier_limit_s': 14400,
                  'stage_prior_execution_s': preparation, 'stage_limit_s': 3600,
                  'cgroup': relative, 'limits': {k: (group / k).read_text().strip() for k in
                  ('memory.max', 'memory.high', 'memory.swap.max', 'cpu.max')},
                  'source_sha256': {str(p.relative_to(ROOT)): digest(p)[0] for p in source_paths},
                  'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
                  'bootstrap': 'exact helper/protocol copies and dependency hashes; no full source snapshot',
                  'before': before, 'transformations': 0}
        write(destination / 'receipt.json', record)
        work = Path(tempfile.mkdtemp(prefix='catalog-reclaim-', dir=scratch_parent))
        record['scratch'] = str(work)
        sys.path.insert(0, str(ROOT / 'tools/bench/labs/completion'))
        import observe
        resources = (destination / 'resources.jsonl').open('x')
        manifest = (destination / 'transformations.jsonl').open('x')
        next_sample = 0.0
        next_admit = 0

        def checkpoint(force=False):
            nonlocal next_sample, next_admit
            if time.monotonic() - started > args.seconds:
                raise TimeoutError('registered deadline exceeded')
            if time.monotonic() >= next_sample or force:
                resources.write(json.dumps(observe.sample(group)) + '\n')
                resources.flush()
                next_sample = time.monotonic() + 5
                if shutil.disk_usage(STORAGE).free < 16 * 1024**3:
                    raise RuntimeError('drive free reserve exhausted')
            if record['transformations'] >= next_admit or force:
                coupled_admit.observe(128 * 1024, 0)
                next_admit = record['transformations'] + 24

        def event(value):
            manifest.write(json.dumps(value) + '\n')
            manifest.flush()
            os.fsync(manifest.fileno())

        def replace(source, canonical, decoded):
            checkpoint()
            a, b = regular(source), regular(canonical)
            if (a.st_dev, a.st_ino) == (b.st_dev, b.st_ino):
                return
            if a.st_dev != b.st_dev:
                raise RuntimeError('hardlink requires same filesystem')
            equal(source, canonical, decoded)
            old, new = digest(source), digest(canonical)
            payload = digest(source, True) if decoded else old
            temporary = source.with_name(source.name + '.catalog-reclaim-owned')
            if temporary.exists() or temporary.is_symlink():
                raise RuntimeError('owned link temporary already exists')
            entry = {'operation': 'decoded_gzip_hardlink' if decoded else 'exact_bytes_hardlink',
                     'path': str(source.relative_to(ROOT)), 'canonical_path': str(canonical.relative_to(ROOT)),
                     'old_compressed_sha256': old[0], 'old_compressed_bytes': old[1],
                     'new_compressed_sha256': new[0], 'new_compressed_bytes': new[1],
                     'decoded_sha256': payload[0], 'decoded_bytes': payload[1],
                     'old_device_inode': [a.st_dev, a.st_ino],
                     'canonical_device_inode': [b.st_dev, b.st_ino],
                     'exact_readback_compared': True, 'state': 'prepared'}
            event(entry)  # Durable provenance BEFORE replacement.
            try:
                os.link(canonical, temporary)
                equal(source, temporary, decoded)
                if digest(source) != old or digest(canonical) != new:
                    raise RuntimeError('source/canonical moved before replacement')
                os.replace(temporary, source)
                equal(source, canonical, decoded)
                entry['state'] = 'replaced'
                event(entry)
                record['transformations'] += 1
            finally:
                if temporary.exists():
                    temporary.unlink()

        def timeout(signum, frame):
            raise TimeoutError('registered180second maximum')

        previous_handler = signal.signal(signal.SIGALRM, timeout)
        signal.alarm(max(1, args.seconds - int(time.monotonic() - started)))
        status = 1
        with (destination / 'stdout.txt').open('w') as out, (destination / 'stderr.txt').open('w') as err:
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                try:
                    checkpoint(True)
                    record['controls'] = controls(work)
                    write(destination / 'receipt.json', record)
                    candidates = collections.defaultdict(list)
                    for path in sorted(COORD.glob('catalog-*/working-tree.diff')):
                        checkpoint()
                        if path.parent == destination:
                            continue
                        terminal(path.parent / 'receipt.json')
                        candidates[digest(path)].append(path)
                    for paths in candidates.values():
                        for path in paths[1:]:
                            replace(path, paths[0], False)
                    # Every old job whose command references a selected dataset
                    # must be terminal/inactive, including failed producer jobs.
                    selected = set()
                    for receipt in sorted(COORD.glob('*/receipt.json')):
                        if receipt.parent == destination:
                            continue
                        row = json.loads(receipt.read_text())
                        if any(name in str(row.get('command', [])) for name in CAPACITY):
                            terminal(receipt)
                            selected.update(name for name in CAPACITY if name in str(row.get('command', [])))
                    if selected != set(CAPACITY):
                        raise RuntimeError('each capacity root needs explicit terminal producer receipt')
                    candidates = collections.defaultdict(list)
                    for name in CAPACITY:
                        folder = BASE / name / 'objects'
                        if folder.is_symlink() or not folder.is_dir():
                            raise RuntimeError('exact capacity object directory required')
                        for path in sorted(folder.iterdir()):
                            checkpoint()
                            regular(path)
                            if path.suffix != '.gz':
                                raise RuntimeError('unexpected capacity object extension')
                            payload = digest(path, True)
                            if not path.name.startswith(payload[0] + '.'):
                                raise RuntimeError('object name differs from decoded payload hash')
                            candidates[payload].append(path)
                    for paths in candidates.values():
                        canonical = min(paths, key=lambda p: (p.stat().st_size, str(p)))
                        for path in paths:
                            replace(path, canonical, True)
                    checkpoint(True)
                    record['after'] = coupled_admit.observe(0, 0)
                    status = 0
                except BaseException as exc:
                    record['error'] = repr(exc)
                    print(repr(exc), file=sys.stderr, flush=True)
                finally:
                    signal.alarm(0)
                    signal.signal(signal.SIGALRM, previous_handler)
                    resources.write(json.dumps(observe.sample(group)) + '\n')
                    resources.close()
                    manifest.close()
                    shutil.rmtree(work)
                    record.update(state='passed' if status == 0 else 'failed', exit=status,
                                  elapsed_s=time.monotonic() - started, scratch_removed=not work.exists(),
                                  cgroup_final={k: (group / k).read_text().strip() for k in
                                  ('memory.peak', 'memory.events', 'memory.swap.current', 'cpu.stat', 'io.stat')})
                    write(destination / 'receipt.json', record)
                    print(json.dumps({k: record[k] for k in ('id', 'state', 'exit', 'elapsed_s', 'transformations')}), flush=True)
        print(json.dumps({k: record[k] for k in ('id', 'state', 'exit', 'elapsed_s', 'transformations')}), flush=True)
        return status


if __name__ == '__main__':
    raise SystemExit(main())
