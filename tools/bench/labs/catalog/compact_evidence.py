#!/usr/bin/env python3
"""Lossless offline catalog evidence compaction; never operates on active slices."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
DATA = ROOT / 'docs/experiments/benchmarks/data'
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits, STORAGE
CHUNK = 1024 * 1024
MAX_DECODED = 1024 * 1024 * 1024


def digest(path, compressed=False):
    result, size = hashlib.sha256(), 0
    opener = gzip.open if compressed else open
    with opener(path, 'rb') as stream:
        while block := stream.read(CHUNK):
            size += len(block)
            if size > MAX_DECODED:
                raise RuntimeError('single evidence payload exceeds 1 GiB bound')
            result.update(block)
    return result.hexdigest(), size


def equal_payload(left, right, left_gzip=True, right_gzip=True):
    """Digest agreement never replaces the exact streaming byte comparison."""
    with (gzip.open if left_gzip else open)(left, 'rb') as a, (gzip.open if right_gzip else open)(right, 'rb') as b:
        while True:
            first, second = a.read(CHUNK), b.read(CHUNK)
            if first != second:
                raise RuntimeError('lossless evidence payload bytes differ')
            if not first:
                return


def compress_json(source, retain_receipt):
    """Closed-file API: callback persists transformation before raw-file removal."""
    source = Path(source)
    if source.is_symlink() or not source.resolve().is_relative_to(DATA) or source.suffix != '.json':
        raise RuntimeError('owned repository JSON evidence required')
    destination = source.with_suffix('.json.gz')
    temporary = destination.with_suffix('.gz.compaction-owned')
    if destination.exists() or temporary.exists():
        raise RuntimeError('compression destination already exists')
    old_sha, old_bytes = digest(source)
    with source.open('rb') as src, temporary.open('xb') as target:
        with gzip.GzipFile(filename='', mode='wb', fileobj=target, mtime=0, compresslevel=6) as zipped:
            shutil.copyfileobj(src, zipped, CHUNK)
    equal_payload(source, temporary, False, True)
    if digest(temporary, True) != (old_sha, old_bytes):
        raise RuntimeError('compressed readback SHA/length differs')
    os.replace(temporary, destination)
    entry = {'operation': 'json_gzip', 'original_path': str(source.resolve().relative_to(ROOT)),
             'retained_path': str(destination.resolve().relative_to(ROOT)),
             'decoded_sha256': old_sha, 'decoded_bytes': old_bytes,
             'new_compressed_sha256': digest(destination)[0],
             'new_compressed_bytes': destination.stat().st_size,
             'exact_readback_compared': True, 'original_removed': False}
    retain_receipt(entry)
    source.unlink()
    entry['original_removed'] = True
    retain_receipt(entry)
    return entry


def files(directory):
    paths = []
    for path in directory.rglob('*'):
        if path.is_symlink():
            raise RuntimeError('evidence symlink forbidden')
        if path.is_file():
            paths.append(path)
    return sorted(paths)


def inventory(directories):
    entries, unique, allocated = [], {}, {}
    for directory in directories:
        for path in files(directory):
            stat = path.stat()
            key = (stat.st_dev, stat.st_ino)
            unique[key] = stat.st_size
            allocated[key] = stat.st_blocks * 512
            entries.append({'path': str(path.relative_to(ROOT)), 'bytes': stat.st_size,
                            'allocated_bytes': stat.st_blocks * 512,
                            'sha256': digest(path)[0], 'device': stat.st_dev, 'inode': stat.st_ino})
    return {'paths': entries, 'logical_path_bytes': sum(row['bytes'] for row in entries),
            'unique_inode_bytes': sum(unique.values()),
            'unique_inode_allocated_bytes': sum(allocated.values()), 'unique_inodes': len(unique)}


def controls(scratch):
    folder = scratch / 'catalog-compaction-controls'
    folder.mkdir()
    good = folder / 'good.gz'
    altered = folder / 'altered.gz'
    truncated = folder / 'truncated.gz'
    with gzip.open(good, 'wb') as stream:
        stream.write(b'actual artifact bytes\n')
    with gzip.open(altered, 'wb') as stream:
        stream.write(b'changed artifact bytes\n')
    truncated.write_bytes(good.read_bytes()[:-5])
    equal_payload(good, good)
    rejected = {}
    for name, bad in [('changed_decoded_bytes', altered), ('truncated_gzip', truncated),
                      ('missing_object', folder / 'missing.gz')]:
        try:
            equal_payload(good, bad)
        except (RuntimeError, EOFError, OSError):
            rejected[name] = True
        else:
            raise RuntimeError('archive checker accepted ' + name)
    shutil.rmtree(folder)
    return rejected


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--inputs', nargs='+', type=Path, required=True)
    parser.add_argument('--account', nargs='*', type=Path, default=[])
    parser.add_argument('--prior', nargs='*', type=Path, default=[])
    parser.add_argument('--pool', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--cap-mib', type=int, choices=[256, 1024], default=256)
    args = parser.parse_args()
    require_limits()
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    if not scratch.is_relative_to(STORAGE / 'scratch'):
        raise RuntimeError('owned mounted scratch required')
    inputs, account, prior = [], [], []
    for group, values in ((inputs, args.inputs), (account, args.account), (prior, args.prior)):
        for value in values:
            if value.is_symlink():
                raise RuntimeError('owned evidence symlink forbidden')
            path = value.resolve(strict=True)
            if not path.is_relative_to(DATA) or not path.is_dir():
                raise RuntimeError('repository data directory required')
            group.append(path)
    if len(set(inputs)) != len(inputs) or any(a != b and a.is_relative_to(b) for a in inputs for b in inputs):
        raise RuntimeError('duplicate or overlapping input directories')
    for directory in inputs:
        if not directory.name.startswith(('catalog-boundary-', 'catalog-maintenance-')):
            raise RuntimeError('only named closed catalog query slices may be transformed')
        cleanup = json.loads((directory / 'cleanup.json').read_text())
        if cleanup.get('status') not in ('complete', 'completed'):
            raise RuntimeError('active/incomplete slice cannot be compacted')
    pool, out = args.pool.resolve(), args.out.resolve()
    if pool.parent != DATA or not pool.name.startswith('catalog-evidence-pool') or pool.is_symlink():
        raise RuntimeError('dedicated repository catalog evidence pool required')
    if out.parent != DATA or not out.name.startswith('catalog-evidence-compaction-') or out.exists():
        raise RuntimeError('fresh dedicated compaction receipt directory required')
    if any(pool.is_relative_to(p) or out.is_relative_to(p) for p in inputs):
        raise RuntimeError('pool/receipt cannot lie within input directories')
    if pool.exists():
        if (pool / 'ownership.json').read_text() != '{"owner":"catalog-evidence-compaction-v1"}\n':
            raise RuntimeError('existing pool owner differs')
    else:
        pool.mkdir()
        (pool / 'ownership.json').write_text('{"owner":"catalog-evidence-compaction-v1"}\n')
    out.mkdir()
    scopes = list(dict.fromkeys([*inputs, *account, pool]))
    record = {'status': 'interrupted', 'command': sys.argv, 'source_sha256': digest(Path(__file__))[0],
              'before': inventory(scopes), 'previous_campaign_separate': inventory(prior),
              'transformations': [], 'controls': controls(scratch),
              'semantic_normalization': False, 'cap_bytes': args.cap_mib * 1024**2}
    receipt = out / 'receipt.json'

    def save():
        receipt.write_text(json.dumps(record, indent=2) + '\n')

    save()
    try:
        for directory in inputs:
            # Freeze the input inventory before mutations; created files are not processed twice.
            for source in files(directory):
                if source.suffix == '.json' and source.stat().st_size >= 1024 * 1024:
                    def keep(entry):
                        if not any(existing is entry for existing in record['transformations']):
                            record['transformations'].append(entry)
                        save()
                    compress_json(source, keep)
                    destination = source.with_suffix('.json.gz')
                elif source.suffix == '.gz':
                    destination = source
                else:
                    continue
                old_compressed_sha, old_compressed_bytes = digest(destination)
                decoded_sha, decoded_bytes = digest(destination, True)
                canonical = pool / (decoded_sha + '.gz')
                if not canonical.exists():
                    shutil.copy2(destination, canonical)
                equal_payload(destination, canonical)
                if digest(canonical, True) != (decoded_sha, decoded_bytes):
                    raise RuntimeError('canonical pool content differs')
                if destination.stat().st_ino != canonical.stat().st_ino or destination.stat().st_dev != canonical.stat().st_dev:
                    temporary = destination.with_suffix('.gz.hardlink-owned')
                    if temporary.exists():
                        raise RuntimeError('owned hardlink temporary already exists')
                    os.link(canonical, temporary)
                    equal_payload(destination, temporary)
                    entry = {'operation': 'gzip_decoded_byte_hardlink', 'path': str(destination.relative_to(ROOT)),
                             'canonical_path': str(canonical.relative_to(ROOT)),
                             'decoded_sha256': decoded_sha, 'decoded_bytes': decoded_bytes,
                             'old_compressed_sha256': old_compressed_sha,
                             'old_compressed_bytes': old_compressed_bytes,
                             'new_compressed_sha256': digest(canonical)[0],
                             'new_compressed_bytes': canonical.stat().st_size,
                             'exact_decoded_bytes_compared': True, 'replaced': False}
                    record['transformations'].append(entry)
                    save()
                    os.replace(temporary, destination)
                    entry['replaced'] = True
                    save()
        record['after'] = inventory(scopes)
        record['previous_campaign_separate_after'] = inventory(prior)
        # Receipt content cannot contain its own hash; account its actual blocks separately.
        record['status'] = 'accounting'
        save()
        for _ in range(8):
            stat = receipt.stat()
            accounted = max(record['after']['unique_inode_bytes'] + stat.st_size,
                            record['after']['unique_inode_allocated_bytes'] + stat.st_blocks * 512)
            record['receipt_bytes'] = stat.st_size
            record['receipt_allocated_bytes'] = stat.st_blocks * 512
            record['aggregate_cap_accounted_bytes'] = accounted
            record['status'] = 'complete' if accounted <= record['cap_bytes'] else 'cap_exceeded_preserved'
            save()
            if receipt.stat().st_size == stat.st_size and receipt.stat().st_blocks == stat.st_blocks:
                break
        else:
            raise RuntimeError('receipt accounting failed to stabilize')
        print(json.dumps({'status': record['status'], 'logical_path_bytes': record['after']['logical_path_bytes'],
                          'unique_inode_bytes': record['after']['unique_inode_bytes'], 'receipt': str(receipt)}))
        if record['status'] != 'complete':
            raise SystemExit(2)
    finally:
        save()


if __name__ == '__main__':
    main()
