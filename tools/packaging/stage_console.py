"""Stage only the complete, hash-verified console from an actual build receipt."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from resource_group import require_limits


ROOT = Path(__file__).resolve().parents[2]


PNG_SIGNATURE = b'\x89PNG\r\n\x1a\n'


def require_timeless_png(name, body):
    """Refuse a PNG carrying time or text metadata: it makes rebuilds differ."""
    if not body.startswith(PNG_SIGNATURE):
        raise ValueError('console icon is not a PNG: ' + name)
    offset = len(PNG_SIGNATURE)
    while offset + 12 <= len(body):
        length = int.from_bytes(body[offset:offset + 4], 'big')
        kind = body[offset + 4:offset + 8]
        if kind in (b'tIME', b'tEXt', b'zTXt', b'iTXt'):
            raise ValueError('console icon carries time or text metadata: ' + name)
        if kind == b'IEND':
            return
        offset += 12 + length
    raise ValueError('truncated console icon: ' + name)


def _sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def _verify_product_source(build, receipt_bytes, product_source, helper_runner, evidence):
    source_root, source_commit, receipt_sha256, helper_snapshot, helper_sha256 = product_source
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True)
    if not source_root.is_absolute() or not re.fullmatch(r'[a-f0-9]{40}', source_commit):
        raise ValueError('explicit product source root and full commit are required')
    if not re.fullmatch(r'[a-f0-9]{64}', receipt_sha256) or not re.fullmatch(r'[a-f0-9]{64}', helper_sha256):
        raise ValueError('explicit console receipt and helper SHA-256 pins are required')
    if (not helper_snapshot.is_absolute() or helper_snapshot.resolve(strict=True) != helper_snapshot
            or not helper_snapshot.is_relative_to(scratch) or not helper_snapshot.is_file()
            or not helper_snapshot.stat().st_mode & 0o111):
        raise ValueError('console source helper snapshot must be executable owned scratch')
    snapshot_sha256 = _sha256(helper_snapshot)
    if snapshot_sha256 != helper_sha256:
        raise ValueError('console source helper snapshot differs from caller pin')
    receipt_path = build / 'build.json'
    if hashlib.sha256(receipt_bytes).hexdigest() != receipt_sha256:
        raise ValueError('console build receipt differs from caller pin')
    evidence.update({'status': 'verifying', 'source_root': str(source_root),
                     'source_commit': source_commit, 'build_root': str(build),
                     'receipt_sha256': receipt_sha256, 'helper_snapshot': str(helper_snapshot),
                     'helper_sha256': helper_sha256})
    evidence.update({'helper_snapshot': str(helper_snapshot), 'helper_snapshot_sha256': snapshot_sha256})
    try:
        report, exit_code, stdout_bytes, stderr_bytes = helper_runner(
            helper_snapshot, source_root, source_commit, build, receipt_sha256)
    except BaseException as error:
        evidence['helper_error'] = str(error)
        raise
    evidence.update({'helper_exit': exit_code, 'helper_stdout_bytes': stdout_bytes,
                     'helper_stderr_bytes': stderr_bytes, 'helper_report': report})
    if exit_code != 0:
        raise ValueError('console source helper failed')
    if _sha256(receipt_path) != receipt_sha256:
        raise ValueError('console build receipt changed after source verification')
    exact = {'status': 'matched', 'source_root': str(source_root),
             'source_commit': source_commit, 'build_root': str(build),
             'receipt_sha256': receipt_sha256}
    if not isinstance(report, dict) or any(report.get(key) != value for key, value in exact.items()):
        raise ValueError('console source helper report identity does not match caller pins')
    count = report.get('source_count')
    source_bytes = report.get('source_bytes_hashed')
    receipt_bytes = report.get('receipt_bytes_hashed')
    total_bytes = report.get('bytes_hashed')
    if (not isinstance(count, int) or isinstance(count, bool) or not 1 <= count <= 512
            or any(not isinstance(value, int) or isinstance(value, bool) or value < 0
                   for value in (source_bytes, receipt_bytes, total_bytes))
            or total_bytes != source_bytes + receipt_bytes):
        raise ValueError('console source helper report has invalid structural counts')
    evidence['status'] = 'matched'


def stage(build, destination, *, product_source=None, helper_runner=None, evidence=None):
    build = Path(build)
    destination = Path(destination)
    receipt_path = build / 'build.json'
    receipt_bytes = receipt_path.read_bytes()
    if product_source is not None:
        if not isinstance(product_source, tuple) or len(product_source) != 5:
            raise ValueError('explicit console product source tuple required')
        receipt_pin = product_source[2]
        if not isinstance(receipt_pin, str) or not re.fullmatch(r'[a-f0-9]{64}', receipt_pin):
            raise ValueError('explicit console receipt SHA-256 pin required')
        if hashlib.sha256(receipt_bytes).hexdigest() != receipt_pin:
            raise ValueError('console build receipt differs from caller pin')
    receipt = json.loads(receipt_bytes)
    if receipt.get('exit') != 0 or not isinstance(receipt.get('assets'), dict):
        raise ValueError('successful console build receipt required')
    if product_source is None:
        sources = receipt.get('source_sha256')
        if not isinstance(sources, dict) or not sources:
            raise ValueError('console source identity required')
        for name, identity in sources.items():
            source = ROOT / name
            if Path(name).is_absolute() or '..' in Path(name).parts or not source.is_file():
                raise ValueError('unsafe or missing console source')
            if hashlib.sha256(source.read_bytes()).hexdigest() != identity:
                raise ValueError('console was built from different source: ' + name)
    else:
        if helper_runner is None or evidence is None:
            raise ValueError('explicit product source verification requires owned helper effects and evidence')
        _verify_product_source(build, receipt_bytes, product_source, helper_runner, evidence)
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
        if name.endswith('.png'):
            require_timeless_png(name, body)
    if sum(identity['bytes'] for identity in assets.values()) > 8 * 1024 * 1024:
        raise ValueError('console exceeds 8 MiB budget')
    if product_source is not None and _sha256(receipt_path) != product_source[2]:
        raise ValueError('console build receipt changed before staging')
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
