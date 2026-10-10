"""Replace only this failed relocation's symlinks with plain location records.

This bounded metadata repair restores the existing ledger's no-symlink rule;
the normal coordinator cannot start while that census rejects the links.
The enclosing resource launcher records its separate maintenance duration.
"""
import json
from pathlib import Path

import service


def main():
    service.resource_group.require_limits()
    root = service.ROOT / 'docs/experiments/benchmarks/data/hammer-reference-01/memory'
    records = json.loads((root / 'failure-archive-location-01/summary.json').read_text())
    if not records['completed'] or len(records['archives']) != 11:
        raise RuntimeError('unexpected relocation record')
    repaired = []
    for item in records['archives']:
        source, target = Path(item['source']).absolute(), Path(item['target'])
        if (source.parent.parent != root / 'failure-preservation-01'
                or source.name != 'failure.tar.gz'
                or target.parent != service.resource_group.STORAGE / 'evidence/hammer-reference-01/failure-preservation-01'
                or not item['verified'] or not source.is_symlink()
                or source.readlink() != target):
            raise RuntimeError('not an exact owned relocation link')
        pointer = source.with_name('failure.location.json')
        with pointer.open('x') as stream:
            json.dump(dict(archive=str(target), archive_bytes=item['bytes'],
                           archive_sha256=item['sha256'], original_archive=str(source)), stream, indent=2)
            stream.write('\n')
        source.unlink()  # Remove the link only; never delete the archive target.
        repaired.append(str(pointer))
    out = root / 'failure-archive-location-01/reference-repair.json'
    out.write_text(json.dumps(dict(repaired=repaired, archives_revalidated=False,
        reason='restore no-symlink evidence census; full revalidation follows'), indent=2)+'\n')
    (out.parent / 'archive_references.py').write_bytes(Path(__file__).read_bytes())
    print(json.dumps(dict(replaced_links=len(repaired), archives_deleted=0)))


if __name__ == '__main__':
    main()
