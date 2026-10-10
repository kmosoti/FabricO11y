#!/usr/bin/env python3
"""Exact-byte, path-preserving deduplication of eight frozen immutable protocol snapshots."""
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

from protocol_snapshot_dedup import regular, identity, metadata, digest, equal, target, fsync_dir, write, controls

ROOT = Path(__file__).resolve().parents[4]
COORD = ROOT / 'docs/experiments/benchmarks/data/lab-completion-run-01/coordinator'
RESERVE = 1280 * 1024
PROTOCOL = ROOT / 'docs/experiments/benchmarks/catalog-protocol-dedup-round2-protocol.md'
# (exact SHA, length, keeper path, frozen logical path count)
GROUPS = (
    ('49fef0c028c709e0929df6c707c3b5b8458df086bb631973f2afc303d2c99f05', 10279, 'catalog-algorithm-checks-01/protocol-40.txt', 198),
    ('1aa7aed1532be128a940ded44d1825fe7628ae5d93473e95fe0411bf1c08395c', 8402, 'catalog-algorithm-checks-01/protocol-32.txt', 194),
    ('edbc48568c5f36263cad448028cab9c81930716179e7143c99786830680c3570', 9090, 'catalog-algorithm-checks-01/protocol-14.txt', 167),
    ('f8d15dd6ff359c009cc40a16b08e770e98b99f34c573f8aaf1af3532555b9d32', 9174, 'catalog-algorithm-checks-01/protocol-28.txt', 160),
    ('391f74ee36aa816127db3eab090de84119ca7fcae98ed5faa0767118aea55d65', 6246, 'catalog-algorithm-checks-01/protocol-4.txt', 199),
    ('cf1da03fc1edc4dfcafe53456c2b540af6792f64e2c47e5deae7bdb841a968cd', 5559, 'catalog-algorithm-checks-01/protocol-6.txt', 199),
    ('edce1b1b09774e282fcc1f59ba6aed4748d2b94cdf82058ba45aa877ded01038', 5473, 'catalog-algorithm-checks-01/protocol-7.txt', 199),
    ('30405ae8b000575a2cf7884b08efcec7dce3d99a7997a4fdff90c0ca3b476bdc', 6429, 'catalog-algorithm-checks-01/protocol-18.txt', 198),
)

def main():
    started = time.monotonic()
    require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--id', default='catalog-protocol-dedup-02')
    parser.add_argument('--seconds', type=int, default=180)
    args = parser.parse_args()
    if args.id != 'catalog-protocol-dedup-02' or not 0 < args.seconds <= 180:
        parser.error('fixed fresh ID and at most180seconds required')
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
            ROOT / 'tools/resource_group.py',
            ROOT / 'tools/bench/labs/catalog/protocol_snapshot_dedup.py']
        for source in dependencies:
            regular(source)
        for source in (Path(__file__).resolve(), PROTOCOL, ROOT / 'tools/bench/labs/catalog/protocol_snapshot_dedup.py'):
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
            snapshots = {}
            for path in sorted(COORD.glob('catalog-*/protocol-*.txt')):
                checkpoint()
                target(path, COORD)
                st = regular(path)
                if st.st_size in {row[1] for row in GROUPS}:
                    snapshots.setdefault((digest(path), st.st_size), []).append((path, st))
            candidates = []
            inactive_units = {}
            summaries = []
            for sha, size, keeper_name, expected_count in GROUPS:
                keeper = COORD / keeper_name
                keeper_stat = regular(keeper)
                if keeper_stat.st_size != size or digest(keeper) != sha:
                    raise RuntimeError('keeper identity/hash/length mismatch')
                members = snapshots.get((sha, size), [])
                inodes = {(st.st_dev, st.st_ino): st for _, st in members}
                if len(members) != expected_count or len(inodes) < 2:
                    raise RuntimeError('frozen group count/distinct inode mismatch')
                if keeper not in [p for p, _ in members]:
                    raise RuntimeError('keeper not covered by group')
                summaries.append(dict(sha256=sha, bytes=size, keeper=keeper_name,
                    paths=len(members), original_distinct_inodes=len(inodes),
                    gross_allocated_reclaim=sum(st.st_blocks * 512 for st in inodes.values()) - keeper_stat.st_blocks * 512))
                for path, st in members:
                    equal(path, keeper)
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
                    candidates.append((path, st, keeper, keeper_stat, sha, size))
            def prepared(row):
                path, original, _, keeper_stat, sha, size = row
                return dict(path=str(path.relative_to(COORD)), sha256=sha, bytes=size,
                            original=metadata(original), keeper=metadata(keeper_stat))
            def completed(row):
                path, _, _, keeper_stat, sha, size = row
                return dict(path=str(path.relative_to(COORD)), state='complete',
                            retained_inode=keeper_stat.st_ino, retained_device=keeper_stat.st_dev,
                            sha256=sha, bytes=size)
            expected = [row for row in candidates
                if (row[1].st_dev, row[1].st_ino) != (row[3].st_dev, row[3].st_ino)]
            projected_manifest = sum(
                len((json.dumps(dict(prepared(row), state='intent'), separators=(',', ':')) + '\n').encode())
                + len((json.dumps(completed(row), separators=(',', ':')) + '\n').encode())
                for row in expected)
            checkpoint(projected_manifest + 4096)
            record['projected_manifest_bytes'] = projected_manifest
            record['groups'] = summaries
            record['gross_allocated_reclaim'] = sum(row['gross_allocated_reclaim'] for row in summaries)
            record['inactive_units_checked'] = len(inactive_units)
            record['inactive_unit_states_sha256'] = hashlib.sha256(
                json.dumps(inactive_units, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
            record['inactive_states'] = sorted(set(inactive_units.values()))
            record['matched_paths'] = len(candidates)
            record['already_keeper_paths'] = [str(row[0].relative_to(COORD)) for row in candidates
                if (row[1].st_dev, row[1].st_ino) == (row[3].st_dev, row[3].st_ino)]
            for row in expected:
                path, original, keeper, keeper_stat, sha, _ = row
                checkpoint()
                if identity(regular(path)) != identity(original) or identity(regular(keeper)) != identity(keeper_stat):
                    raise RuntimeError('original stat identity changed')
                if original.st_dev != keeper_stat.st_dev:
                    raise RuntimeError('same filesystem required')
                equal(path, keeper)
                temporary = path.with_name(path.name + '.protocol-dedup-round2-owned')
                if temporary.exists() or temporary.is_symlink():
                    raise RuntimeError('owned temporary already exists')
                event(dict(prepared(row), state='intent'))
                try:
                    os.link(keeper, temporary)
                    equal(path, temporary)
                    if identity(regular(path)) != identity(original) or identity(regular(keeper)) != identity(keeper_stat):
                        raise RuntimeError('identity moved before replacement')
                    os.replace(temporary, path)
                    fsync_dir(path.parent)
                    equal(path, keeper)
                    if identity(regular(path)) != identity(keeper_stat) or digest(path) != sha:
                        raise RuntimeError('retained inode/bytes mismatch')
                    event(completed(row))
                    record['transformations'] += 1
                finally:
                    if temporary.exists():
                        temporary.unlink()
                        fsync_dir(path.parent)
            manifest.flush()
            os.fsync(manifest.fileno())
            rows = [json.loads(line) for line in (destination / 'transformations.jsonl').read_text().splitlines()]
            if len(rows) != 2 * len(expected) or record['transformations'] != len(expected):
                raise RuntimeError('manifest coverage/count mismatch')
            for index, row in enumerate(expected):
                if rows[2 * index] != dict(prepared(row), state='intent') or rows[2 * index + 1] != completed(row):
                    raise RuntimeError('manifest intent/completion mismatch')
            for path, _, keeper, keeper_stat, sha, _ in candidates:
                checkpoint()
                target(path, COORD)
                equal(path, keeper)
                if identity(regular(path)) != identity(keeper_stat) or digest(path) != sha:
                    raise RuntimeError('final path byte/inode mismatch')
                temporary = path.with_name(path.name + '.protocol-dedup-round2-owned')
                if temporary.exists() or temporary.is_symlink():
                    raise RuntimeError('orphan owned temporary')
            record['manifest_readback'] = dict(intent_pairs=len(expected), completed_pairs=len(expected),
                verified_candidate_paths=len(candidates), keeper_paths_accounted=len(GROUPS), orphan_owned_temporaries=False)
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
