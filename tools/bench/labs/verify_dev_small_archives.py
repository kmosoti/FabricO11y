#!/usr/bin/env python3
"""Verify preserved screen bytes and lossless shared ledgers after cleanup."""
import gzip
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits

DATA = ROOT / 'docs/experiments/benchmarks/data/dev-small-labs-run-02'


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def verify_manifest(root, entries):
    for name, expected in entries.items():
        path = (root / name).resolve()
        if not path.is_relative_to(root.resolve()) or sha(path) != expected:
            raise ValueError('archive hash mismatch: ' + name)


def main():
    require_limits()
    # Representative alteration must fail despite the same manifest shape.
    own = Path(__file__).resolve()
    verify_manifest(own.parent, {own.name: sha(own)})
    try:
        verify_manifest(own.parent, {own.name: '0' * 64})
    except ValueError:
        pass
    else:
        raise AssertionError('altered archive hash was accepted')
    checked = {}
    for lab in ('memory', 'query'):
        labdir = DATA / lab
        cells = {}
        for path in sorted(labdir.iterdir()):
            manifest = path / 'artifact-manifest.json'
            if not manifest.exists():
                continue
            entries = json.loads(manifest.read_text())
            verify_manifest(path, entries)
            cells[path.name] = len(entries)
        corpora = {}
        for path in sorted((labdir / 'corpus').glob('*.jsonl.gz')):
            with gzip.open(path, 'rb') as stream:
                digest = hashlib.file_digest(stream, 'sha256').hexdigest()
            if digest != path.name.removesuffix('.jsonl.gz'):
                raise ValueError('shared corpus hash mismatch: ' + str(path))
            corpora[path.name] = digest
        size = sum(path.stat().st_size for path in labdir.rglob('*') if path.is_file())
        if size > 50 * 2**20:
            raise ValueError('retained lab evidence exceeds bound: ' + lab)
        checked[lab] = {'cell_manifest_entries': cells, 'corpora': corpora, 'bytes': size}
    result = {'passed': True, 'altered_hash_control_rejected': True, 'labs': checked,
              'scope': 'Retained file/corpus hashes and evidence size; lossless source/clock round trips already ran before every successful cleanup.'}
    destination = DATA / 'archive-verification.json'
    with destination.open('x') as stream:
        json.dump(result, stream, indent=2)
        stream.write('\n')
    print(json.dumps(result))


if __name__ == '__main__':
    main()
