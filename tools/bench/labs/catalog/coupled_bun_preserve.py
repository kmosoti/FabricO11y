#!/usr/bin/env python3
"""Preserve one closed Bun-only failure by an already-retained archive member."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import tarfile
import tempfile

import coupled_admit
from coupled_cleanup import inactive, source_manifest, sync_copy
from resource_group import STORAGE, require_limits

ROOT = Path(__file__).resolve().parents[4]
BASE = ROOT / 'docs/experiments/benchmarks/data/lab-completion-run-01'
ID = 'catalog-coupled-bun-preserve-01'
JOB = 'catalog-coupled-continuation-closeout-01'
RELATIVE = 'coordinator/' + JOB + '/tmp/bun'
ARCHIVE = 'coordinator/failure-catalog-docs-01.tar.gz'
MEMBER = 'coordinator/catalog-docs-01/tmp/bun'
ARCHIVE_SHA = '60707809e50853d3690c36218f9941f48cf8adf5d0bf81213f2d35b9cd4cfede'
PAYLOAD_SHA = 'a83d263767d839e4d2649ca8e35d07159c7afc99afdc96d731ced29e056dda0c'
PAYLOAD_BYTES = 79_500_640


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def regular(path, boundary):
    if not path.is_relative_to(boundary):
        raise RuntimeError('path outside exact ownership boundary')
    for part in (path, *path.parents):
        if part.is_symlink():
            raise RuntimeError('linked ownership path')
        if part == boundary:
            break
    if not path.is_file():
        raise RuntimeError('regular file required')


def compare(archive, member, source, expected_sha, expected_bytes):
    """One implementation for real preservation and representative controls."""
    if archive.is_symlink() or source.is_symlink():
        raise RuntimeError('linked payload rejected')
    with tarfile.open(archive, 'r:gz') as stream:
        members = [item for item in stream.getmembers() if item.name == member]
        if len(members) != 1 or not members[0].isfile() or members[0].size != expected_bytes:
            raise RuntimeError('missing/ambiguous/nonregular archive member')
        digest, count = hashlib.sha256(), 0
        with stream.extractfile(members[0]) as original, source.open('rb') as current:
            while True:
                first, second = original.read(1024 * 1024), current.read(1024 * 1024)
                if first != second:
                    raise RuntimeError('archive member and source bytes differ')
                if not first:
                    break
                count += len(first)
                digest.update(first)
        if (digest.hexdigest(), count) != (expected_sha, expected_bytes):
            raise RuntimeError('decoded archive identity mismatch')


def controls(work):
    import io
    archive = work / 'control.tar.gz'
    source = work / 'source'
    source.write_bytes(b'actual bytes')
    expected = hashlib.sha256(source.read_bytes()).hexdigest()
    with tarfile.open(archive, 'x:gz') as output:
        member = tarfile.TarInfo('member')
        member.size = source.stat().st_size
        output.addfile(member, io.BytesIO(source.read_bytes()))
    compare(archive, 'member', source, expected, 12)
    rejected = []
    for name in ('changed_source', 'missing_member'):
        source.write_bytes(b'changed data' if name == 'changed_source' else b'actual bytes')
        try:
            compare(archive, 'absent' if name == 'missing_member' else 'member', source, expected, 12)
        except RuntimeError:
            rejected.append(name)
        else:
            raise RuntimeError('preservation control accepted: ' + name)
    return rejected


def persist(path, report):
    temporary = path.with_suffix('.writing')
    if temporary.is_symlink():
        raise RuntimeError('linked report temporary')
    with temporary.open('w') as output:
        output.write(json.dumps(report, indent=2) + '\n')
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, path)
    directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def main():
    require_limits()
    report_path = BASE / (ID + '.json')
    if report_path.exists() or report_path.is_symlink():
        raise RuntimeError('fresh preservation report required')
    scratch = Path(os.environ['TMPDIR']).resolve(strict=True)
    if not scratch.is_relative_to(STORAGE / 'scratch') or not STORAGE.parent.is_mount():
        raise RuntimeError('owned mounted scratch required')
    coupled_admit.observe(4 * 1024**2, 0)
    report = {'id': ID, 'state': 'running', 'jobs': []}
    persist(report_path, report)
    try:
        with tempfile.TemporaryDirectory(prefix='catalog-bun-controls-', dir=scratch) as name:
            report['controls'] = controls(Path(name))
        receipt = BASE / 'coordinator' / JOB / 'receipt.json'
        regular(receipt, ROOT)
        row = json.loads(receipt.read_text())
        if row.get('id') != JOB or row.get('state') != 'failed':
            raise RuntimeError('exact failed closeout receipt required')
        unit = Path(row['cgroup']).name.removesuffix('.service')
        if unit != 'fabric-work-c5399335b00d4c049583f42fd0291acc':
            raise RuntimeError('unexpected failed source unit')
        inactive(unit)
        outer = ROOT / 'target/resource-containment/runs' / (unit + '.json')
        regular(outer, ROOT)
        outer_row = json.loads(outer.read_text())
        retained = STORAGE / 'evidence' / unit
        if outer_row.get('retained_failure_evidence') != str(retained):
            raise RuntimeError('outer receipt does not own exact retained root')
        if retained.is_symlink() or not retained.is_dir():
            raise RuntimeError('exact retained directory required')
        expected = {RELATIVE: {'bytes': PAYLOAD_BYTES, 'sha256': PAYLOAD_SHA}}
        if source_manifest(retained) != expected:
            raise RuntimeError('Bun must be sole regular file with exact bytes')
        source = retained / RELATIVE
        regular(source, retained)
        archive = BASE / ARCHIVE
        manifest = BASE / 'coordinator/failure-catalog-docs-01.tar.manifest.json'
        regular(archive, ROOT)
        regular(manifest, ROOT)
        if sha(archive) != ARCHIVE_SHA:
            raise RuntimeError('canonical archive hash mismatch')
        if json.loads(manifest.read_text()).get(MEMBER) != expected[RELATIVE]:
            raise RuntimeError('canonical old member manifest mismatch')
        compare(archive, MEMBER, source, PAYLOAD_SHA, PAYLOAD_BYTES)
        copies = BASE / 'coordinator/launcher-receipts'
        if copies.is_symlink():
            raise RuntimeError('linked receipt directory')
        copies.mkdir(exist_ok=True)
        copied = copies / (JOB + '.json')
        if copied.is_symlink():
            raise RuntimeError('linked receipt copy')
        if copied.exists():
            if copied.read_bytes() != outer.read_bytes():
                raise RuntimeError('existing outer receipt copy differs')
        else:
            sync_copy(outer, copied)
        if copied.read_bytes() != outer.read_bytes():
            raise RuntimeError('outer receipt copy mismatch')
        entry = {'id': JOB, 'preservation_kind': 'archive_member_reference',
                 'byte_verified': True, 'scratch_removed': False,
                 'source_manifest': expected, 'archive': ARCHIVE,
                 'archive_sha256': ARCHIVE_SHA, 'member': MEMBER,
                 'decoded_sha256': PAYLOAD_SHA, 'decoded_bytes': PAYLOAD_BYTES,
                 'outer_receipt': str(copied.relative_to(BASE)),
                 'outer_receipt_sha256': sha(copied), 'ready_to_delete': True}
        report['jobs'] = [entry]
        report['state'] = 'ready_to_delete'
        report['before_delete'] = coupled_admit.observe(0, 0)
        persist(report_path, report)
        # Check the entire original tree and canonical immediately before removal.
        if source_manifest(retained) != expected or sha(archive) != ARCHIVE_SHA:
            raise RuntimeError('source or canonical moved after durable reference')
        inactive(unit)
        shutil.rmtree(retained)
        entry['scratch_removed'] = not retained.exists()
        report['state'] = 'completed'
        report['after'] = coupled_admit.observe(0, 0)
        persist(report_path, report)
        print(json.dumps(report), flush=True)
    except BaseException as error:
        report['state'] = 'failed'
        report['error'] = repr(error)
        persist(report_path, report)
        raise


if __name__ == '__main__':
    main()
