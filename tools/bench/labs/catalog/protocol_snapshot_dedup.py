#!/usr/bin/env python3
"""Exact-byte, path-preserving deduplication of one immutable protocol snapshot."""
import argparse
import fcntl
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
COORD = ROOT / 'docs/experiments/benchmarks/data/lab-completion-run-01/coordinator'
KEEPER = COORD / 'catalog-plan-docs-01/protocol-0.txt'
SHA = '49736fe0c829d9d86751b913b62892612b1b7a605f045572dac1d2c5be783fd9'
SIZE = 29418
RESERVE = 256 * 1024
PROTOCOL = ROOT / 'docs/experiments/benchmarks/catalog-protocol-dedup-protocol.md'


def regular(path):
    for parent in (path, *path.parents):
        if parent == ROOT:
            break
        if parent.is_symlink():
            raise RuntimeError('indirect evidence ownership')
    st = path.lstat()
    if not stat.S_ISREG(st.st_mode):
        raise RuntimeError('nonregular evidence')
    return st


def identity(st):
    return (st.st_dev, st.st_ino, st.st_size, st.st_mode, st.st_uid, st.st_gid, st.st_mtime_ns)


def metadata(st):
    return dict(device=st.st_dev, inode=st.st_ino, bytes=st.st_size,
                mode=st.st_mode, uid=st.st_uid, gid=st.st_gid,
                mtime_ns=st.st_mtime_ns, allocated_bytes=st.st_blocks * 512)


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def equal(left, right):
    regular(left)
    regular(right)
    with left.open('rb') as a, right.open('rb') as b:
        while True:
            x, y = a.read(32768), b.read(32768)
            if x != y:
                raise RuntimeError('byte comparison rejected')
            if not x:
                return


def target(path, base):
    if (path.parent.parent != base or len(path.parent.name) > 64 or not re.fullmatch(r'catalog-[a-z0-9_-]+', path.parent.name)
            or not re.fullmatch(r'protocol-[0-9]+\.txt', path.name)):
        raise RuntimeError('incorrect target')


def fsync_dir(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def write(path, value):
    temporary = path.with_name(path.name + '.writing')
    with temporary.open('w') as stream:
        json.dump(value, stream, indent=2)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)
    fsync_dir(path.parent)


def controls(work):
    good, bad = work / 'good', work / 'bad'
    good.write_bytes(b'original')
    bad.write_bytes(b'altered!')
    equal(good, good)
    rejected = []
    for name, action in (
            ('corrupted_bytes', lambda: equal(good, bad)),
            ('incorrect_target', lambda: target(work / 'catalog-control/source.tar.gz', work))):
        try:
            action()
        except RuntimeError:
            rejected.append(name)
        else:
            raise RuntimeError('negative control accepted: ' + name)
    return rejected


def main():
    started = time.monotonic()
    require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--id', default='catalog-protocol-dedup-01')
    parser.add_argument('--seconds', type=int, default=120)
    args = parser.parse_args()
    if args.id != 'catalog-protocol-dedup-01' or not 0 < args.seconds <= 120:
        parser.error('fixed fresh ID and at most120seconds required')
    regular(PROTOCOL)
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True)
    if not scratch.is_relative_to((STORAGE / 'scratch').resolve(strict=True)) or not STORAGE.parent.is_mount():
        raise RuntimeError('owned mounted data-drive scratch required')
    if shutil.disk_usage(STORAGE).free < 16 * 1024**3:
        raise RuntimeError('16GiB disk reserve required')
    with (ROOT / 'target/readiness-lab.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        receipts = [json.loads(p.read_text()) for p in COORD.glob('*/receipt.json')]
        if any(r.get('state') == 'running' for r in receipts):
            raise RuntimeError('coordinator job still running')
        consumed = sum(r.get('elapsed_s', 0) for r in receipts)
        frontier = sum(r.get('elapsed_s', 0) for r in receipts if r['id'].startswith(('catalog-', 'frontier-')))
        preparation = sum(r.get('elapsed_s', 0) for r in receipts if r.get('stage') == 'preparation')
        if min(86400 - consumed, 14400 - frontier, 3600 - preparation) < args.seconds:
            raise RuntimeError('full deadline does not fit existing budgets')
        if int(os.environ.get('FABRIC_RESOURCE_RUNTIME_SECONDS', '0')) < args.seconds + 30:
            raise RuntimeError('outer deadline needs30seconds cleanup margin')
        destination = COORD / args.id
        if destination.exists() or destination.is_symlink():
            raise RuntimeError('fresh receipt required')
        before = coupled_admit.observe(RESERVE, 0)
        destination.mkdir()
        dependencies = [Path(__file__).resolve(), PROTOCOL,
            ROOT / 'tools/bench/labs/catalog/coupled_admit.py',
            ROOT / 'tools/bench/labs/catalog/campaign_summary.py',
            ROOT / 'tools/bench/labs/catalog/coupled_resource_readiness.py',
            ROOT / 'tools/bench/labs/catalog/coupled_cleanup.py',
            ROOT / 'tools/resource_group.py']
        for source in dependencies:
            regular(source)
        for source in (Path(__file__).resolve(), PROTOCOL):
            copied = destination / ('protocol.txt' if source == PROTOCOL else source.name)
            shutil.copyfile(source, copied)
            equal(source, copied)
            with copied.open('rb') as stream:
                os.fsync(stream.fileno())
        relative = next(line[3:] for line in Path('/proc/self/cgroup').read_text().splitlines() if line.startswith('0::'))
        group = Path('/sys/fs/cgroup') / relative.lstrip('/')
        record = dict(id=args.id, lab='coordinator', stage='preparation', state='running',
            command=sys.argv, cwd=str(ROOT), started_unix_ns=time.time_ns(), timeout_s=args.seconds,
            prior_execution_s=consumed, frontier_prior_execution_s=frontier, frontier_limit_s=14400,
            stage_prior_execution_s=preparation, stage_limit_s=3600, cgroup=relative,
            limits={k: (group / k).read_text().strip() for k in ('memory.max', 'memory.high', 'memory.swap.max', 'cpu.max')},
            cgroup_before={k: (group / k).read_text().strip() for k in ('memory.current', 'memory.peak', 'cpu.stat', 'io.stat')},
            source_sha256={str(p.relative_to(ROOT)): digest(p) for p in dependencies},
            revision=subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
            before=before, reservation_bytes=RESERVE, transformations=0,
            assumption='terminal protocol copies are immutable; existing lock provides a single writer',
            metadata_policy='original metadata retained in manifest; linked paths acquire keeper metadata')
        write(destination / 'receipt.json', record)
        work = Path(tempfile.mkdtemp(prefix='protocol-dedup-', dir=scratch))
        record['scratch'] = str(work)
        manifest = (destination / 'transformations.jsonl').open('x')
        status = 1
        def checkpoint(extra=0):
            if time.monotonic() - started > args.seconds:
                raise TimeoutError('registered deadline exceeded')
            allocated = destination.stat().st_blocks * 512 + sum(p.stat().st_blocks * 512 for p in destination.iterdir())
            # 32KiB remains for final receipt/failed scratch/atomic receipt rewrite.
            if allocated + extra + 32768 > RESERVE:
                raise RuntimeError('prospective output/failure reservation exhausted')
        def event(row):
            raw = json.dumps(row, separators=(',', ':')) + '\n'
            checkpoint(len(raw.encode()) + 4096)
            manifest.write(raw)
            manifest.flush()
            os.fsync(manifest.fileno())
        def timeout(signum, frame):
            raise TimeoutError('registered deadline exceeded')
        previous = signal.signal(signal.SIGALRM, timeout)
        signal.alarm(max(1, int(args.seconds - (time.monotonic() - started))))
        try:
            record['negative_controls_rejected'] = controls(work)
            keeper_stat = regular(KEEPER)
            if keeper_stat.st_size != SIZE or digest(KEEPER) != SHA:
                raise RuntimeError('keeper identity/hash/length mismatch')
            candidates = []
            inactive_units = {}
            for path in sorted(COORD.glob('catalog-*/protocol-*.txt')):
                checkpoint()
                target(path, COORD)
                st = regular(path)
                if st.st_size == SIZE and digest(path) == SHA:
                    equal(path, KEEPER)
                    producer_path = path.parent / 'receipt.json'
                    regular(producer_path)
                    producer = json.loads(producer_path.read_text())
                    if producer.get('id') != path.parent.name or producer.get('state') not in ('passed', 'failed', 'interrupted'):
                        raise RuntimeError('snapshot producer is not terminal')
                    unit = Path(producer.get('cgroup', '')).name.removesuffix('.service')
                    if not re.fullmatch(r'fabric-work-[0-9a-f]{32}', unit):
                        raise RuntimeError('unknown producer cgroup unit')
                    if unit not in inactive_units:
                        inactive_units[unit] = inactive(unit)
                    candidates.append((path, st))
            if len({(st.st_dev, st.st_ino) for _, st in candidates}) < 2:
                raise RuntimeError('at least two distinct original inodes required')
            def prepared(path, original):
                return dict(path=str(path.relative_to(COORD)), sha256=SHA, bytes=SIZE,
                            original=metadata(original), keeper=metadata(keeper_stat))
            def completed(path):
                return dict(path=str(path.relative_to(COORD)), state='complete',
                            retained_inode=keeper_stat.st_ino, retained_device=keeper_stat.st_dev,
                            sha256=SHA, bytes=SIZE)
            projected_manifest = sum(
                len((json.dumps(dict(prepared(p, st), state='intent'), separators=(',', ':')) + '\n').encode())
                + len((json.dumps(completed(p), separators=(',', ':')) + '\n').encode())
                for p, st in candidates if (st.st_dev, st.st_ino) != (keeper_stat.st_dev, keeper_stat.st_ino))
            checkpoint(projected_manifest + 4096)
            record['projected_manifest_bytes'] = projected_manifest
            record['inactive_units_checked'] = len(inactive_units)
            record['inactive_unit_states_sha256'] = hashlib.sha256(
                json.dumps(inactive_units, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
            record['inactive_states'] = sorted(set(inactive_units.values()))
            record['matched_paths'] = len(candidates)
            record['already_keeper_paths'] = [str(p.relative_to(COORD)) for p, st in candidates
                if (st.st_dev, st.st_ino) == (keeper_stat.st_dev, keeper_stat.st_ino)]
            record['keeper'] = str(KEEPER.relative_to(ROOT))
            for path, original in candidates:
                if (original.st_dev, original.st_ino) == (keeper_stat.st_dev, keeper_stat.st_ino):
                    continue
                checkpoint()
                if identity(regular(path)) != identity(original) or identity(regular(KEEPER)) != identity(keeper_stat):
                    raise RuntimeError('original stat identity changed')
                if original.st_dev != keeper_stat.st_dev:
                    raise RuntimeError('same filesystem required')
                equal(path, KEEPER)
                temporary = path.with_name(path.name + '.protocol-dedup-owned')
                if temporary.exists() or temporary.is_symlink():
                    raise RuntimeError('owned temporary already exists')
                entry = prepared(path, original)
                event(dict(entry, state='intent'))
                try:
                    os.link(KEEPER, temporary)
                    equal(path, temporary)
                    if identity(regular(path)) != identity(original) or identity(regular(KEEPER)) != identity(keeper_stat):
                        raise RuntimeError('identity moved before replacement')
                    os.replace(temporary, path)
                    fsync_dir(path.parent)
                    equal(path, KEEPER)
                    if identity(regular(path)) != identity(keeper_stat) or digest(path) != SHA:
                        raise RuntimeError('retained inode/bytes mismatch')
                    event(completed(path))
                    record['transformations'] += 1
                finally:
                    if temporary.exists():
                        temporary.unlink()  # Only this exact owned link.
                        fsync_dir(path.parent)
            manifest.flush()
            os.fsync(manifest.fileno())
            rows = [json.loads(line) for line in (destination / 'transformations.jsonl').read_text().splitlines()]
            expected = [(p, st) for p, st in candidates
                        if (st.st_dev, st.st_ino) != (keeper_stat.st_dev, keeper_stat.st_ino)]
            if len(rows) != 2 * len(expected) or record['transformations'] != len(expected):
                raise RuntimeError('manifest coverage/count mismatch')
            for index, (path, original) in enumerate(expected):
                if rows[2 * index] != dict(prepared(path, original), state='intent') or rows[2 * index + 1] != completed(path):
                    raise RuntimeError('manifest intent/completion mismatch')
            # Read every logical candidate back, including the keeper and any
            # paths that already shared its inode before this operation.
            for path, _ in candidates:
                checkpoint()
                target(path, COORD)
                equal(path, KEEPER)
                if identity(regular(path)) != identity(keeper_stat) or digest(path) != SHA:
                    raise RuntimeError('final path byte/inode mismatch')
                temporary = path.with_name(path.name + '.protocol-dedup-owned')
                if temporary.exists() or temporary.is_symlink():
                    raise RuntimeError('orphan owned temporary')
            record['manifest_readback'] = dict(intent_pairs=len(expected), completed_pairs=len(expected),
                verified_candidate_paths=len(candidates), keeper_accounted=True, orphan_owned_temporaries=False)
            record['after'] = coupled_admit.observe(0, 0)
            status = 0
        except BaseException as exc:
            record['error'] = repr(exc)
            print(repr(exc), file=sys.stderr, flush=True)
        finally:
            signal.alarm(0)
            signal.signal(signal.SIGALRM, previous)
            manifest.close()
            # Scratch contains only our two tiny control files. Copy them even
            # on failure before cleanup; no source evidence ever enters scratch.
            if status != 0:
                for path in work.iterdir():
                    shutil.copyfile(path, destination / ('failure-control-' + path.name))
            shutil.rmtree(work)
            record.update(state='passed' if status == 0 else 'failed', exit=status,
                elapsed_s=time.monotonic() - started, scratch_removed=not work.exists(),
                cgroup_final={k: (group / k).read_text().strip() for k in
                              ('memory.current', 'memory.peak', 'memory.events', 'memory.swap.current', 'cpu.stat', 'io.stat')})
            write(destination / 'receipt.json', record)
            print(json.dumps({k: record[k] for k in ('id', 'state', 'exit', 'elapsed_s', 'transformations')}), flush=True)
        return status


if __name__ == '__main__':
    raise SystemExit(main())
