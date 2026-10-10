#!/usr/bin/env python3
"""Read-only source-evidence checks and deterministic rejection controls.

This does not repair interrupted retrievals or prove upstream source correctness.
Run through resource_group.py; no networking, extraction or upstream execution.
"""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import tarfile
import tempfile
from unittest import mock

from source_fetch import require_limits, archive_readback, resolve
import source_supplement


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def immutable_pin(pin):
    if not isinstance(pin, str) or re.fullmatch('[0-9a-f]{40}', pin) is None:
        raise ValueError('revision is not an immutable40hex pin')


def safe_name(name):
    path = PurePosixPath(name)
    if path.is_absolute() or '..' in path.parts or not path.parts or str(path) != name:
        raise ValueError('unsafe or noncanonical archive path')


def archive_check(stream, expected):
    seen, raw_bytes = set(), 0
    with tarfile.open(fileobj=stream, mode='r:gz') as archive:
        for member in archive:
            safe_name(member.name)
            if not member.isfile() or member.name in seen or member.name not in expected:
                raise ValueError('nonregular, duplicate or unlisted archive member')
            receipt = expected[member.name]
            if member.size != receipt['bytes']:
                raise ValueError('archive member size differs from receipt')
            payload = archive.extractfile(member).read()
            if len(payload) != receipt['bytes'] or sha(payload) != receipt['sha256']:
                raise ValueError('archive bytes differ from receipt')
            seen.add(member.name)
            raw_bytes += len(payload)
    if seen != set(expected):
        raise ValueError('missing manifest member, including any recorded failed prefix')
    return {'members': len(seen), 'raw_bytes': raw_bytes, 'duplicate_and_type_checks': True}


def fetch_check(folder):
    receipt = json.loads((folder / 'receipt.json').read_text())
    immutable_pin(receipt['revision'])
    if receipt['status'] in ('retrieval failed', 'retrieval interrupted'):
        prefix = folder / 'failed-download.tar.gz'
        if receipt.get('failed_download_bytes', 0):
            payload = prefix.read_bytes()
            if len(payload) != receipt['failed_download_bytes'] or sha(payload) != receipt['failed_download_sha256']:
                raise ValueError('failed archive prefix differs from receipt')
        return {'status': receipt['status'], 'prefix_checked': prefix.exists()}
    expected = receipt['selected_members']
    with (folder / 'selected-source.tar.gz').open('rb') as stream:
        result = archive_check(stream, expected)
    inventory, seen = [], set()
    with gzip.open(folder / 'regular-members.jsonl.gz', 'rt') as stream:
        for line in stream:
            row = json.loads(line)
            safe_name(row['path'])
            if row['path'] in seen:
                raise ValueError('duplicate inventory path')
            seen.add(row['path'])
            inventory.append(row)
    if (len(inventory) != receipt['regular_members']
            or sum(row['bytes'] for row in inventory) != receipt['regular_member_bytes']):
        raise ValueError('inventory coverage differs from receipt')
    indexed = {row['path']: row for row in inventory}
    if any(indexed.get(name) != dict(path=name, **row) for name, row in expected.items()):
        raise ValueError('selected member differs from full inventory')
    for name, field in [('selected-source.tar.gz', 'selected_archive_sha256'),
                        ('regular-members.jsonl.gz', 'inventory_sha256')]:
        if sha((folder / name).read_bytes()) != receipt[field]:
            raise ValueError('retained artifact digest differs from receipt')
    return result


def supplement_check(folder):
    receipts = json.loads((folder / 'receipt.json').read_text())
    expected, failed_prefixes = {}, 0
    for identity, record in receipts.items():
        safe_name(identity)
        immutable_pin(record['revision'])
        for name, row in record['files'].items():
            safe_name(name)
            url = f"https://raw.githubusercontent.com/{record['repository']}/{record['revision']}/{name}"
            if row['url'] != url:
                raise ValueError('file URL differs from recorded immutable pin')
            if row['bytes']:
                expected[identity + '/' + name] = {'bytes': row['bytes'], 'sha256': row['sha256']}
                failed_prefixes += bool(row['exit'])
    with (folder / 'selected-source.tar.gz').open('rb') as stream:
        result = archive_check(stream, expected)
    summary = json.loads((folder / 'summary.json').read_text())
    if sha((folder / 'selected-source.tar.gz').read_bytes()) != summary['archive_sha256']:
        raise ValueError('supplement archive digest differs from summary')
    if result['members'] != summary['readback_files']:
        raise ValueError('supplement member count differs from summary')
    return dict(result, failed_prefixes_checked=failed_prefixes)


def controls():
    payload = b'exact source prefix\n'
    expected = {'repo/src/file.rs': {'bytes': len(payload), 'sha256': sha(payload)}}
    def fixture(entries):
        out = io.BytesIO()
        with tarfile.open(fileobj=out, mode='w:gz') as archive:
            for name, raw, kind in entries:
                member = tarfile.TarInfo(name)
                member.type = kind
                member.size = len(raw) if kind == tarfile.REGTYPE else 0
                archive.addfile(member, io.BytesIO(raw) if kind == tarfile.REGTYPE else None)
        out.seek(0)
        return out
    valid = [('repo/src/file.rs', payload, tarfile.REGTYPE)]
    archive_check(fixture(valid), expected)
    rejected = {}
    cases = {'duplicate': valid + valid, 'missing_failed_prefix': [],
             'changed': [('repo/src/file.rs', b'wrong source prefix\n', tarfile.REGTYPE)],
             'absolute': [('/src/file.rs', payload, tarfile.REGTYPE)],
             'traversal': [('repo/../file.rs', payload, tarfile.REGTYPE)],
             'symlink': [('repo/src/file.rs', b'', tarfile.SYMTYPE)]}
    for name, entries in cases.items():
        try:
            archive_check(fixture(entries), expected)
        except ValueError as error:
            rejected[name] = str(error)
        else:
            raise ValueError('negative control accepted: ' + name)
        try:
            archive_readback(fixture(entries), {'repo/src/file.rs': payload})
        except RuntimeError:
            rejected['production_readback_' + name] = True
        else:
            raise ValueError('production archive readback accepted: ' + name)
    for pin in ('main', '', 'a' * 39):
        try:
            immutable_pin(pin)
        except ValueError:
            rejected['mutable_or_invalid_pin_' + repr(pin)] = True
        else:
            raise ValueError('mutable pin accepted')
        for producer, check in [('fetch', lambda: resolve('repo', pin, Path('.'), 0)),
                                ('supplement', lambda: source_supplement.validate_pin(pin))]:
            try:
                check()
            except RuntimeError:
                rejected[producer + '_invalid_pin_' + repr(pin)] = True
            else:
                raise ValueError(producer + ' accepted mutable pin')
    interrupted = preservation_control()
    return {'origin': 'source retrieval preservation review; deterministic archive fixtures',
            'valid_archive_checked': True, 'rejected': rejected,
            'injected_interruption': interrupted}


def preservation_control():
    """Inject interruption after a completed failed prefix; require durable bytes."""
    owned = tempfile.mkdtemp(prefix='source-controls-', dir=os.environ['FABRIC_SCRATCH_ROOT'])
    try:
        base = Path(owned)
        class Pipe(io.BytesIO):
            def __init__(self, fd):
                super().__init__()
                self.fd = fd
            def fileno(self):
                return self.fd
        class Child:
            stdout, stderr, returncode = Pipe(100), Pipe(101), None
            def poll(self):
                return self.returncode
            def kill(self):
                self.returncode = -9
            def wait(self, **kwargs):
                return self.returncode
        # An oversized fake response exercises the actual bounded streaming code.
        with mock.patch.object(source_supplement.subprocess, 'Popen', return_value=Child()), \
                mock.patch.object(source_supplement.select, 'select', side_effect=lambda active, *args: (active, [], [])), \
                mock.patch.object(source_supplement.os, 'read', return_value=b'0123456789abcdef'), \
                mock.patch.object(source_supplement, 'FILE_LIMIT', 8):
            bounded = source_supplement.download_file('https://fixture.invalid', base / 'bounded.source', base / 'bounded.stderr')
        if ((base / 'bounded.source').read_bytes() != b'01234567' or bounded['bytes'] != 8
                or not bounded['exit'] or not bounded['prefix_truncated']):
            raise ValueError('bounded download did not preserve exact8-byte failed prefix')
        (base / 'repo').mkdir()
        (base / 'repo/receipt.json').write_text(json.dumps({'repository': 'owner/repo', 'revision': 'a' * 40}))
        prefix = b'bounded failed response prefix\n'
        calls = []
        def fake_download(url, target, error):
            calls.append(url)
            if len(calls) == 2:
                target.write_bytes(b'current partial prefix')
                error.write_bytes(b'injected interruption')
                raise KeyboardInterrupt('source-controls deterministic cut after first receipt')
            target.write_bytes(prefix)
            error.write_bytes(b'injected curl failure with retained bytes')
            return {'url': url, 'argv': ['fixture'], 'exit': 22, 'bytes': len(prefix),
                    'sha256': sha(prefix), 'error': 'injected', 'staged_source': str(target),
                    'staged_error': str(error)}
        with mock.patch.object(source_supplement, 'BASE', base), \
                mock.patch.object(source_supplement, 'REQUESTS', {'repo': ['one.rs', 'two.rs']}), \
                mock.patch.object(source_supplement, 'require_limits', lambda: None), \
                mock.patch.object(source_supplement, 'download_file', fake_download), \
                mock.patch('sys.argv', ['source_supplement.py']), \
                mock.patch.dict(os.environ, FABRIC_SCRATCH_ROOT=str(base)):
            try:
                source_supplement.main()
            except KeyboardInterrupt:
                pass
            else:
                raise ValueError('injected source interruption did not propagate')
        records = json.loads((base / 'supplements-01/receipt.json').read_text())
        state = json.loads((base / 'supplements-01/staging.json').read_text())
        first, second = records['repo']['files']['one.rs'], records['repo']['files']['two.rs']
        if (Path(first['staged_source']).read_bytes() != prefix or first['exit'] != 22
                or first['sha256'] != sha(prefix) or second['state'] != 'running'
                or Path(second['staged_source']).read_bytes() != b'current partial prefix'
                or state['state'] != 'interrupted' or state['scratch_removed']):
            raise ValueError('interruption lost full staging or durable per-file receipt')
        shutil.rmtree(base)
        return {'first_failed_prefix_sha256': sha(prefix), 'first_receipt_durable': True,
                'current_partial_prefix_preserved': True, 'full_stage_retained': True,
                'streaming_prefix_bound_checked_bytes': bounded['bytes'],
                'fixture_storage': 'launcher-owned data drive; generated fixture cleaned after verification'}
    except BaseException as error:
        raise RuntimeError(f'preservation control failed; full fixture retained at {owned}: {error!r}') from error


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--controls', action='store_true')
    parser.add_argument('--fetch', type=Path, nargs='*', default=[])
    parser.add_argument('--supplements', type=Path, nargs='*', default=[])
    args = parser.parse_args()
    require_limits()
    if not (args.controls or args.fetch or args.supplements):
        parser.error('select controls or explicit evidence directories')
    result = {}
    if args.controls:
        result['controls'] = controls()
    for path in args.fetch:
        result[str(path)] = fetch_check(path)
    for path in args.supplements:
        result[str(path)] = supplement_check(path)
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
