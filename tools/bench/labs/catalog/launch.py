"""Build/freeze new lab examples and run their registered isolated screens."""
import argparse
import gzip
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits, STORAGE


def main():
    require_limits()
    parser = argparse.ArgumentParser()
    parser.add_argument('kind', choices=['spill', 'metadata'])
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    base = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True)
    if not base.is_relative_to(STORAGE / 'scratch'):
        raise RuntimeError('owned data-drive scratch required')
    if args.out.exists():
        raise RuntimeError('fresh result path required')
    if shutil.disk_usage(STORAGE).free < 16 * 1024**3:
        raise RuntimeError('16 GiB free-space reserve required')
    work = base / ('catalog-' + args.kind + '-binaries')
    work.mkdir()
    example = 'catalog_' + args.kind + '_probe'
    frozen = {}
    for variant in ('plain', 'counted'):
        command = ['cargo', 'build', '--offline', '--locked', '--release', '-p',
                   'fabric-server', '--example', example]
        if variant == 'counted':
            command += ['--features', 'responsibility-alloc-probe']
        print(json.dumps({'build': variant, 'command': command}), flush=True)
        subprocess.run(command, check=True, timeout=600)
        frozen[variant] = work / variant
        shutil.copy2(Path(os.environ['CARGO_TARGET_DIR']) / 'release/examples' / example,
                     frozen[variant])
    command = [sys.executable, '-B', str(Path(__file__).with_name(args.kind + '.py')),
               '--plain', str(frozen['plain']), '--counted', str(frozen['counted']),
               '--out', str(args.out)]
    result = subprocess.run(command, timeout=900)
    if args.out.is_dir():
        archive = args.out / 'binaries'
        archive.mkdir()
        for variant, binary in frozen.items():
            with binary.open('rb') as source, gzip.open(archive / (variant + '.gz'), 'wb') as dest:
                shutil.copyfileobj(source, dest)
    if result.returncode:
        raise SystemExit(result.returncode)
    if sum(p.stat().st_size for p in args.out.rglob('*') if p.is_file()) > 256 * 1024**2:
        raise RuntimeError('lab evidence exceeds 256 MiB; preserve scratch')
    shutil.rmtree(work)
    (args.out / 'binary-cleanup.json').write_text(json.dumps({'removed': True,
        'scratch': str(work), 'archived': ['binaries/plain.gz', 'binaries/counted.gz']}) + '\n')


if __name__ == '__main__':
    main()
