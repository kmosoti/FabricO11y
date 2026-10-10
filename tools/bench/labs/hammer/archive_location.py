"""Relocate authenticated bulky failure archives to owned data-drive evidence."""
import hashlib
import json
import os
from pathlib import Path
import sys
import time

import service


def main():
    service.resource_group.require_limits()
    root = service.ROOT / 'docs/experiments/benchmarks/data/hammer-reference-01/memory'
    source_root = root / 'failure-preservation-01'
    summary = source_root / 'summary.json'
    rows = json.loads(summary.read_text())
    if not rows or sum(row['archive_bytes'] for row in rows) > 6 * 2**30:
        raise RuntimeError('archive count/aggregate exceeds relocation scope')
    out = root / Path(os.environ['FABRIC_SCRATCH_ROOT']).name
    out.mkdir(mode=0o700, exist_ok=False)
    (out / 'archive_location.py').write_bytes(Path(__file__).read_bytes())
    target_root = service.resource_group.STORAGE / 'evidence/hammer-reference-01/failure-preservation-01'
    target_root.mkdir(mode=0o700, parents=True, exist_ok=False)
    deadline = time.monotonic() + 110
    report = dict(source_summary=str(summary), source_summary_sha256=service.digest(summary),
                  destination=str(target_root), archives=[], completed=False)

    def checkpoint():
        path = out / 'summary.json'
        with path.open('w') as stream:
            json.dump(report, stream, indent=2)
            stream.write('\n')
            stream.flush()
            os.fsync(stream.fileno())

    def transfer(stream, output=None):
        h = hashlib.sha256()
        while True:
            if time.monotonic() > deadline:
                raise TimeoutError('archive relocation deadline')
            block = stream.read(1024 * 1024)
            if not block:
                return h.hexdigest()
            h.update(block)
            if output is not None:
                output.write(block)

    checkpoint()
    try:
        for row in rows:
            source = Path(row['archive'])
            expected_source = source_root / row['unit'] / 'failure.tar.gz'
            if (source.resolve() != expected_source.resolve() or source.is_symlink()
                    or not row.get('removed') or source.stat().st_size != row['archive_bytes']):
                raise RuntimeError('unexpected archive identity/state')
            if source.stat().st_size > 3 * 2**30:
                raise RuntimeError('archive exceeds registered per-tree bound')
            before = source.stat()
            target = target_root / (row['unit'] + '.tar.gz')
            item = dict(source=str(source), target=str(target), bytes=row['archive_bytes'],
                        sha256=row['archive_sha256'], verified=False, repository_reference_replaced=False)
            report['archives'].append(item)
            checkpoint()
            with source.open('rb') as inp, target.open('xb') as output:
                copied_hash = transfer(inp, output)
                output.flush()
                os.fsync(output.fileno())
            with target.open('rb') as stream:
                readback_hash = transfer(stream)
            after = source.stat()
            if (copied_hash != row['archive_sha256'] or readback_hash != copied_hash
                    or (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
                    != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)):
                raise RuntimeError('archive changed or destination readback differs')
            item['verified'] = True
            checkpoint()
            pointer = source.with_name('failure.location.json')
            with pointer.open('x') as stream:
                json.dump(dict(archive=str(target), archive_bytes=item['bytes'],
                               archive_sha256=item['sha256'], original_archive=str(source.absolute())),
                          stream, indent=2)
                stream.write('\n')
                stream.flush()
                os.fsync(stream.fileno())
            source.unlink()
            item['repository_reference_replaced'] = not source.exists() and pointer.is_file()
            if not item['repository_reference_replaced']:
                raise RuntimeError('repository location record was not installed')
            checkpoint()
        report['completed'] = True
    finally:
        checkpoint()
    print(json.dumps(dict(completed=True, archives=len(rows), bytes=sum(r['archive_bytes'] for r in rows))))


if __name__ == '__main__':
    main()
