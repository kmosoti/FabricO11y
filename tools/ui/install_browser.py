"""Fetch a fixed Chrome for Testing and matching driver into the mounted data drive."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[2]
DATA = Path('/run/media/kmosoti/data/FabricO11y')
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits

VERSION = '155.0.8059.39'
BASE = f'https://storage.googleapis.com/chrome-for-testing-public/{VERSION}/linux64/'
PINNED = {
    'chrome-linux64': (198195769, '55672d1f392fd3e7b7a08621b6e804e6bcb39d40cf155504abb74b3a021ea8ea'),
    'chromedriver-linux64': (11879709, 'a65f025c692e686ec2a21fa6b9639c69fa9813683a47af9cabf1ba0b1f453e42'),
}


def main():
    require_limits()
    target = DATA / 'tools/ui' / f'chrome-for-testing-{VERSION}'
    if target.exists():
        raise SystemExit('refuse existing installation; use recorded binaries')
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']) / 'chrome-download'
    scratch.mkdir()
    target.mkdir()
    receipt = {'version': VERSION, 'source': 'https://googlechromelabs.github.io/chrome-for-testing/',
               'archives': {}, 'status': 'interrupted', 'storage': str(target),
               'checksum_scope': 'observed archive SHA-256, not an independent upstream signature'}
    try:
        total = 0
        for name in ('chrome-linux64', 'chromedriver-linux64'):
            archive = scratch / (name + '.zip')
            url = BASE + archive.name
            digest = hashlib.sha256()
            size = 0
            with urllib.request.urlopen(url, timeout=60) as response, archive.open('wb') as output:
                while chunk := response.read(1024 * 1024):
                    size += len(chunk)
                    if size > 300 * 1024 * 1024:
                        raise RuntimeError('archive exceeds 300 MiB download bound')
                    output.write(chunk)
                    digest.update(chunk)
            receipt['archives'][name] = {'url': url, 'sha256': digest.hexdigest(), 'bytes': size}
            if (size,digest.hexdigest()) != PINNED[name]:
                raise RuntimeError('archive differs from reviewed version/hash pin')
            with zipfile.ZipFile(archive) as source:
                for item in source.infolist():
                    path = (target / item.filename).resolve()
                    if not path.is_relative_to(target) or ((item.external_attr >> 16) & 0o170000) == 0o120000:
                        raise RuntimeError('unsafe archive path/type')
                    total += item.file_size
                    if total > 1024 * 1024 * 1024:
                        raise RuntimeError('extracted tools exceed 1 GiB bound')
                    source.extract(item, target)
                    if not item.is_dir():
                        path.chmod(0o755 if item.external_attr >> 16 & 0o111 else 0o644)
            archive.unlink()
        receipt['extracted_bytes'] = total
        receipt['status'] = 'downloaded; executables not yet run'
    except BaseException as error:
        receipt['status'] = 'failed'
        receipt['error'] = str(error)
        raise
    finally:
        shutil.rmtree(scratch)
        receipt['cleanup'] = 'owned archive/extraction scratch removed; reviewed tool installation retained'
        (target / 'download.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps({'version': VERSION, 'storage': str(target), 'extracted_bytes': total}))


if __name__ == '__main__':
    main()
