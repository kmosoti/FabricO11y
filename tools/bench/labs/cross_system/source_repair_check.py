"""Independently replay complete upstream inventories, with corruption controls."""
import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import tarfile

from source_evidence_check import archive_check, controls, immutable_pin, safe_name
from source_fetch import require_limits, digest, ROOT


def bind_selected(inventory, selected):
    for name, record in selected.items():
        if inventory.get(name) != dict(path=name, **record):
            raise ValueError('selected excerpt differs from complete pinned inventory')


def inventory_match(archive, expected):
    seen = set()
    total = 0
    with tarfile.open(fileobj=archive, mode='r|gz') as stream:
        for member in stream:
            if not member.isfile():
                continue
            safe_name(member.name)
            parts = member.name.split('/', 1)
            if len(parts) != 2:
                raise ValueError('missing upstream directory prefix')
            name = parts[1]
            if name in seen or name not in expected:
                raise ValueError('duplicate or unlisted upstream member')
            info = expected[name]
            if member.size != info['bytes']:
                raise ValueError('upstream size changed')
            total += member.size
            if total > 4 * 1024**3:
                raise ValueError('decoded inventory ceiling')
            content = stream.extractfile(member)
            h = hashlib.sha256()
            while block := content.read(1024**2):
                h.update(block)
            if h.hexdigest() != info['sha256']:
                raise ValueError('upstream content changed')
            seen.add(name)
    if seen != set(expected):
        raise ValueError('missing upstream member')
    return dict(members=len(seen), bytes=total)


def corruption_controls():
    def fixture(rows):
        out = io.BytesIO()
        with tarfile.open(fileobj=out, mode='w:gz') as stream:
            for name, payload in rows:
                info = tarfile.TarInfo(name)
                info.size = len(payload)
                stream.addfile(info, io.BytesIO(payload))
        out.seek(0)
        return out
    valid = [('root/a', b'abc')]
    expected = {'a': dict(bytes=3, sha256=hashlib.sha256(b'abc').hexdigest())}
    inventory_match(fixture(valid), expected)
    cases = dict(changed=[('root/a', b'abd')], missing=[],
                 duplicate=valid + valid, traversal=[('root/../a', b'abc')],
                 unlisted=valid + [('root/b', b'abc')])
    for name, entries in cases.items():
        try:
            inventory_match(fixture(entries), expected)
        except ValueError:
            continue
        raise RuntimeError('negative control accepted: ' + name)
    bind_selected({'a': dict(path='a', **expected['a'])}, expected)
    substituted = {'a': dict(bytes=3, sha256=hashlib.sha256(b'abd').hexdigest())}
    try:
        bind_selected({'a': dict(path='a', **expected['a'])}, substituted)
    except ValueError:
        pass
    else:
        raise RuntimeError('self-consistent substituted excerpt accepted')
    return sorted(cases) + ['self_consistent_excerpt_substitution']


def main():
    require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    args = parser.parse_args()
    result = dict(controls=controls(), inventory_controls=corruption_controls(), repositories={})
    for name in ('clickhouse-inventory-retry', 'vector', 'foundationdb'):
        folder = args.source / name
        receipt = json.loads((folder / 'receipt.json').read_text())
        if receipt['state'] != 'passed' or receipt['exit_code'] != 0:
            raise ValueError('source retrieval did not pass')
        immutable_pin(receipt['revision'])
        archive = ROOT / receipt['archive_reference'] if 'archive_reference' in receipt else folder / 'upstream.tar.gz'
        if digest(archive) != receipt['archive_sha256'] or archive.stat().st_size != receipt['archive_bytes']:
            raise ValueError('complete archive digest/size differs')
        inventory = {}
        with gzip.open(folder / 'regular-members.jsonl.gz', 'rt') as stream:
            for line in stream:
                row = json.loads(line)
                safe_name(row['path'])
                if row['path'] in inventory:
                    raise ValueError('duplicate inventory path')
                inventory[row['path']] = row
        if digest(folder / 'regular-members.jsonl.gz') != receipt['inventory_sha256']:
            raise ValueError('inventory digest differs')
        with archive.open('rb') as stream:
            checked = inventory_match(stream, inventory)
        if checked['members'] != receipt['regular_members'] or checked['bytes'] != receipt['regular_member_bytes']:
            raise ValueError('inventory totals differ')
        bind_selected(inventory, receipt['selected_members'])
        if digest(folder / 'selected-source.tar.gz') != receipt['selected_archive_sha256']:
            raise ValueError('selected archive digest differs')
        with (folder / 'selected-source.tar.gz').open('rb') as stream:
            selected = archive_check(stream, receipt['selected_members'])
        result['repositories'][name] = dict(revision=receipt['revision'], full_inventory=checked, selected=selected)
    result['checker_sha256'] = digest(Path(__file__))
    output = args.source / ('verification-' + result['checker_sha256'][:16] + '.json')
    output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))


if __name__ == '__main__':
    main()
