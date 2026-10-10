"""Contained one-time CQ2 builds and immutable executable/source receipts."""
import argparse
import gzip
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import STORAGE, require_limits
spec = importlib.util.spec_from_file_location('profile', ROOT / 'tools/bench/labs/completion/profile.py')
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--seconds', type=int, default=1200)
    args = parser.parse_args()
    require_limits()
    if not 0 < args.seconds <= 1500:
        parser.error('seconds must be 1..1500')
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    out = args.out.resolve()
    if not scratch.is_relative_to(STORAGE / 'scratch') or not out.is_relative_to(ROOT / 'docs/experiments/benchmarks/data') or out.exists():
        raise RuntimeError('fresh repository evidence and mounted owned scratch required')
    out.mkdir()
    work = scratch / 'catalog-maintenance-freeze'
    work.mkdir()
    deadline = time.monotonic() + args.seconds
    source_paths = ['crates/fabric-server/examples/catalog_maintenance_probe.rs',
                    'crates/fabric-server/src/query.rs', 'crates/fabric-server/src/read_catalog.rs',
                    'crates/fabric-server/src/tail.rs', 'crates/fabric-server/src/segment.rs',
                    'tools/bench/labs/catalog/maintenance.py', 'tools/bench/labs/catalog/maintenance_freeze.py',
                    'tools/bench/labs/catalog/compact_evidence.py', 'tools/bench/labs/completion/profile.py',
                    'tools/qualification/query_oracle.py', 'Cargo.lock',
                    'docs/experiments/benchmarks/catalog-maintenance-protocol.md',
                    'docs/experiments/benchmarks/catalog-maintenance-fixture-correction-protocol.md']
    hashes = {name: profile.grader.sha(ROOT / name) for name in source_paths}
    manifest = {'status': 'interrupted', 'source_hashes': hashes, 'binaries': {}, 'commands': [],
                'environment': {'FABRIC_BORROWED_LOG_EXPERIMENT': '0'}, 'argv': sys.argv}

    def save():
        (out / 'freeze.json').write_text(json.dumps(manifest, indent=2) + '\n')

    save()
    try:
        env = dict(os.environ, FABRIC_BORROWED_LOG_EXPERIMENT='0')
        env.pop('FABRIC_SPILL_WORKSPACE_EXPERIMENT', None)
        for variant in ('plain', 'counted'):
            argv = ['cargo', 'build', '--offline', '--locked', '--release', '-p', 'fabric-server',
                    '--example', 'catalog_maintenance_probe']
            if variant == 'counted':
                argv += ['--features', 'responsibility-alloc-probe']
            rc = profile.run_child(argv, env, out / f'{variant}.stdout', out / f'{variant}.stderr',
                                   min(deadline, time.monotonic() + 600), work, out)
            manifest['commands'].append({'argv': argv, 'exit': rc})
            save()
            if rc:
                raise RuntimeError(f'{variant} build exit {rc}')
            if hashes != {name: profile.grader.sha(ROOT / name) for name in source_paths}:
                raise RuntimeError('source changed across frozen build capture')
            source = Path(os.environ['CARGO_TARGET_DIR']) / 'release/examples/catalog_maintenance_probe'
            frozen = work / variant
            shutil.copy2(source, frozen)
            archive = out / f'{variant}.gz'
            with frozen.open('rb') as src, archive.open('xb') as dest:
                with gzip.GzipFile(filename='', fileobj=dest, mode='wb', mtime=0) as zipped:
                    shutil.copyfileobj(src, zipped)
            decoded = work / f'{variant}-readback'
            with gzip.open(archive, 'rb') as src, decoded.open('xb') as dest:
                shutil.copyfileobj(src, dest)
            if decoded.read_bytes() != frozen.read_bytes():
                raise RuntimeError('frozen binary archive bytes changed')
            manifest['binaries'][variant] = {'archive': archive.name,
                                            'decoded_sha256': profile.grader.sha(frozen),
                                            'archive_sha256': profile.grader.sha(archive),
                                            'decoded_bytes': frozen.stat().st_size}
            save()
        manifest['status'] = 'complete'
        save()
        shutil.rmtree(work)
    finally:
        (out / 'cleanup.json').write_text(json.dumps({'status': manifest['status'], 'removed': not work.exists(),
                                                    'scratch': str(work)}) + '\n')


if __name__ == '__main__':
    main()
