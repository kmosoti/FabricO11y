"""Registered retry of three incomplete upstream archives; no source execution."""
import argparse
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile
import time

import source_fetch as fetch

MIB = 1024**2
CAP = 1024 * MIB
PINS = {
    'clickhouse': '47907285810e618994aa703c31beb0edfc5d2271',
    'vector': '47e05c4749b6b1af460af521279dc4681a912bb3',
    'foundationdb': 'daae46ce69b1fdddefdbb985e03e5a12576b3077',
}


def main():
    fetch.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--destination', type=Path, required=True)
    parser.add_argument('--repos', nargs='+', choices=PINS, default=list(PINS))
    parser.add_argument('--reuse-clickhouse', action='store_true')
    args = parser.parse_args()
    data = Path('/run/media/kmosoti/data/FabricO11y').resolve()
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    if not scratch.is_relative_to(data):
        raise RuntimeError('required disk-backed scratch unavailable')
    out = args.destination.resolve()
    registered = fetch.ROOT / 'docs/experiments/benchmarks/data/cross-system-sweep-01/source'
    if out != registered.resolve():
        raise RuntimeError('unregistered source destination')
    out.mkdir(parents=True, exist_ok=True)
    fetch.COMPRESSED = 512 * MIB
    fetch.DECODED = 4 * 1024**3
    fetch.INVENTORY = 16 * MIB
    specs = {s['id']: s for s in json.loads(fetch.CATALOG.read_text())['repositories']}
    deadline = time.monotonic() + 1650
    for name in args.repos:
        if fetch.allocated_size(out) + 537 * MIB > CAP:
            raise RuntimeError('cannot preserve next worst-case bounded archive')
        if shutil.disk_usage(scratch).free < fetch.RESERVE:
            raise RuntimeError('data-drive free reserve unavailable')
        reuse = name == 'clickhouse' and args.reuse_clickhouse
        dest = out / (name + '-inventory-retry' if reuse else name)
        dest.mkdir()
        work = Path(tempfile.mkdtemp(prefix='repair-' + name + '-', dir=scratch))
        archive = work / 'upstream.tar.gz'
        started = time.monotonic()
        receipt = dict(id=name, repository=specs[name]['repo'], revision=PINS[name],
            upstream_code_executed=False, recursive_submodule_checkout=False,
            source_fetch_sha256=fetch.digest(Path(fetch.__file__)),
            repair_sha256=fetch.digest(Path(__file__)), scratch=str(work),
            limits=dict(compressed_bytes=fetch.COMPRESSED, decoded_bytes=fetch.DECODED,
                        selected_bytes=fetch.SELECTED, inventory_bytes=fetch.INVENTORY,
                        network_seconds=300), state='running')
        fetch.dump(dest / 'receipt.json', receipt)
        for script in (Path(__file__), Path(fetch.__file__), fetch.CATALOG):
            shutil.copyfile(script, dest / script.name)
        try:
            if reuse:
                old = out / 'clickhouse'
                previous = json.loads((old / 'receipt.json').read_text())
                original = old / 'failed-download.tar.gz'
                if (previous['revision'] != PINS[name]
                        or previous.get('archive_sha256') != fetch.digest(original)
                        or previous.get('archive_bytes') != original.stat().st_size):
                    raise RuntimeError('complete archive reuse authentication failed')
                # Read the verified retained archive in place; preserve its failed receipt.
                archive = original
                receipt['archive_reference'] = str(original.relative_to(fetch.ROOT))
            else:
                fetch.download(specs[name]['repo'], PINS[name], archive, dest,
                               min(deadline, started + 300))
            receipt.update(archive_bytes=archive.stat().st_size,
                           archive_sha256=fetch.digest(archive))
            receipt.update(fetch.census(archive, specs[name], dest, deadline))
            if not reuse:
                retained = dest / 'upstream.tar.gz'
                shutil.copyfile(archive, retained)
                if fetch.digest(retained) != receipt['archive_sha256']:
                    raise RuntimeError('complete archive preservation mismatch')
                archive.unlink()
            work.rmdir()
            receipt.update(state='passed', exit_code=0, scratch_removed=True)
            print(name, receipt['archive_bytes'], receipt['regular_members'], flush=True)
        except BaseException as error:
            receipt.update(state='failed', exit_code=1, error=repr(error))
            if archive.exists() and not reuse and (dest / 'upstream.tar.gz').exists():
                # A failed preservation copy must not allocate a second full copy.
                # Leave the complete original in owned scratch for the launcher.
                partial = dest / 'upstream.tar.gz'
                receipt.update(preservation_partial_bytes=partial.stat().st_size,
                               preservation_partial_sha256=fetch.digest(partial),
                               retained_original_in_scratch=True)
            elif archive.exists() and not reuse:
                retained = dest / 'failed-download.tar.gz'
                shutil.copyfile(archive, retained)
                if fetch.digest(archive) != fetch.digest(retained):
                    raise RuntimeError('failed archive preservation mismatch')
                receipt.update(failed_download_bytes=retained.stat().st_size,
                               failed_download_sha256=fetch.digest(retained))
                archive.unlink()
            if not any(work.iterdir()):
                work.rmdir()
            receipt['scratch_removed'] = not work.exists()
            raise
        finally:
            receipt.update(elapsed_s=time.monotonic()-started,
                           retained_allocated_bytes=fetch.allocated_size(dest))
            fetch.dump(dest / 'receipt.json', receipt)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
