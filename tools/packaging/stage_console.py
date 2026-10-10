"""Stage only the complete, hash-verified console from an actual build receipt."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from resource_group import require_limits


ROOT = Path(__file__).resolve().parents[2]


def stage(build, destination):
    receipt = json.loads((build / 'build.json').read_text())
    if receipt.get('exit') != 0 or not isinstance(receipt.get('assets'), dict):
        raise ValueError('successful console build receipt required')
    sources = receipt.get('source_sha256')
    if not isinstance(sources, dict) or not sources:
        raise ValueError('console source identity required')
    for name, identity in sources.items():
        source = ROOT / name
        if Path(name).is_absolute() or '..' in Path(name).parts or not source.is_file():
            raise ValueError('unsafe or missing console source')
        if hashlib.sha256(source.read_bytes()).hexdigest() != identity:
            raise ValueError('console was built from different source: ' + name)
    assets = receipt['assets']
    if not {'index.html', 'service-worker.js', 'manifest.webmanifest', 'icon-192.png', 'icon-512.png'} <= assets.keys():
        raise ValueError('complete PWA assets required')
    if not any(name.endswith('.wasm') for name in assets):
        raise ValueError('compiled WASM required')
    if set(p.name for p in (build / 'dist').iterdir()) != assets.keys():
        raise ValueError('asset inventory differs from build receipt')
    for name, identity in assets.items():
        source = build / 'dist' / name
        if Path(name).name != name or source.is_symlink() or not source.is_file():
            raise ValueError('unsafe asset path')
        body = source.read_bytes()
        if len(body) != identity['bytes'] or hashlib.sha256(body).hexdigest() != identity['sha256']:
            raise ValueError('asset differs from console build receipt: ' + name)
    if sum(identity['bytes'] for identity in assets.values()) > 8 * 1024 * 1024:
        raise ValueError('console exceeds 8 MiB budget')
    destination.mkdir(parents=True, exist_ok=False)
    for name in assets:
        shutil.copyfile(build / 'dist' / name, destination / name)
        (destination / name).chmod(0o644)
    (destination / 'console-headers.json').write_text(json.dumps(receipt['headers'], sort_keys=True) + '\n')
    (destination / 'asset-manifest.json').write_text(json.dumps(assets, sort_keys=True) + '\n')


if __name__ == '__main__':
    require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('build', type=Path)
    parser.add_argument('destination', type=Path)
    args = parser.parse_args()
    stage(args.build.resolve(), args.destination)
