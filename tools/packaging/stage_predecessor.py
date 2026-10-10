"""Construct only the registered historical-binary RPM migration fixture stage."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from resource_group import require_limits

REGISTERED_SHA256 = 'ac71995b3c8b88cbb6f7dd05d9854e347b7a783caadc1b602f6590d6460f4a2f'
ROOT = Path(__file__).resolve().parents[2]


def stage_predecessor(package, destination):
    if hashlib.sha256(package.read_bytes()).hexdigest() != REGISTERED_SHA256:
        raise ValueError('predecessor differs from registered historical Debian artifact')
    destination.mkdir(parents=True, exist_ok=False)
    raw = subprocess.check_output(['ar', 'p', str(package), 'data.tar.xz'])
    identities = {}
    with tarfile.open(fileobj=io.BytesIO(raw)) as archive:
        for member in archive.getmembers():
            name = member.name.removeprefix('./')
            allowed = (name.startswith(('usr/bin/', 'usr/lib/systemd/system/', 'usr/lib/sysusers.d/',
                                        'usr/share/doc/fabrico11y/examples/')))
            if not allowed or not member.isfile():
                continue
            if Path(name).is_absolute() or '..' in Path(name).parts or member.size > 32 * 1024 * 1024:
                raise ValueError('unsafe predecessor archive entry')
            content = archive.extractfile(member).read()
            target = destination / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(content)
            target.chmod(0o755 if name.startswith('usr/bin/') else 0o644)
            identities[name] = hashlib.sha256(content).hexdigest()
    for binary in ('fabric-node', 'fabric-server', 'fabricctl'):
        if 'usr/bin/' + binary not in identities:
            raise ValueError('historical binary missing')
    docs = destination / 'usr/share/doc/fabrico11y'
    for name in ('LICENSE', 'NOTICE'):
        (docs / name).write_bytes((ROOT / name).read_bytes())
    (docs / 'MIGRATION-FIXTURE.json').write_text(json.dumps({
        'role': 'constructed RPM predecessor fixture; no historical Fedora release claim',
        'historical_deb_sha256': REGISTERED_SHA256, 'payload_sha256': identities,
        'license_role': 'owner-selected Apache-2.0 project license',
    }, indent=2) + '\n')
    control = subprocess.check_output(['ar', 'p', str(package), 'control.tar.xz'])
    with tarfile.open(fileobj=io.BytesIO(control)) as archive:
        (destination / 'historical-preinst').write_bytes(archive.extractfile('./preinst').read())


if __name__ == '__main__':
    require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('package', type=Path)
    parser.add_argument('destination', type=Path)
    args = parser.parse_args()
    stage_predecessor(args.package.resolve(strict=True), args.destination)
