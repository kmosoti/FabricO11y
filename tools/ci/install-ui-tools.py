"""Fetch hash-pinned UI builders/drivers into the mounted data drive."""
import hashlib
import io
import json
from pathlib import Path
import sys
import tarfile
import urllib.request
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from resource_group import require_limits

TOOLS = {
    'trunk': ('https://github.com/trunk-rs/trunk/releases/download/v0.21.14/trunk-x86_64-unknown-linux-gnu.tar.gz', 'f2b4680cd239693a646a2795e4633c625328d7b2a044fbe749fa3a2fe9e7036b', 'd0e3c0238c8cbecff9109e47e193e0acca2bf66f5243783dc2769ded9feddee1'),
    'geckodriver': ('https://github.com/mozilla/geckodriver/releases/download/v0.37.1/geckodriver-v0.37.1-linux64.tar.gz', 'e815130ea95983e162ae91843b48d3a3ce991735635fce83a647afde21e09f7e', 'f831b7e61454804e8a307edd951bf8a5f373efe3f718e75454a6485a51f6e39f'),
}

if __name__ == '__main__':
    require_limits()
    destination = Path('/run/media/kmosoti/data/FabricO11y/tools/ui')
    destination.mkdir(parents=True, exist_ok=True)
    for name, (url, archive_hash, binary_hash) in TOOLS.items():
        with urllib.request.urlopen(url, timeout=60) as response:
            archive = response.read(64 * 1024 * 1024 + 1)
        if hashlib.sha256(archive).hexdigest() != archive_hash:
            raise SystemExit('UI tool archive hash mismatch: ' + name)
        with tarfile.open(fileobj=io.BytesIO(archive)) as package:
            member = next(m for m in package.getmembers() if Path(m.name).name == name and m.isfile())
            binary = package.extractfile(member).read()
        if hashlib.sha256(binary).hexdigest() != binary_hash:
            raise SystemExit('UI tool binary hash mismatch: ' + name)
        (destination / name).write_bytes(binary)
        (destination / name).chmod(0o755)
        (destination / (name + '.json')).write_text(json.dumps({'url': url, 'archive_sha256': archive_hash, 'binary_sha256': binary_hash}) + '\n')
