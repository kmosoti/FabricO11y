"""Stage the exact alpha2 Debian payload for native RPM metadata construction."""
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


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def validate_control(control):
    fields = dict(line.split(': ', 1) for line in control.splitlines() if ': ' in line and not line.startswith(' '))
    if fields.get('Package') != 'fabrico11y' or fields.get('Version') != '0.1.0~alpha.2' or fields.get('Architecture') != 'amd64':
        raise ValueError('candidate Debian metadata must be exact alpha2 amd64')


def stage_candidate(package, receipt_path, destination):
    receipt = json.loads(receipt_path.read_text())
    if receipt.get('state') != 'built' or receipt.get('package_family') != 'debian' or not receipt.get('container_cleanup_confirmed'):
        raise ValueError('candidate needs a completed successful isolated Debian build receipt')
    if package.is_symlink() or digest(package) != receipt.get('package_sha256') or package.resolve() != Path(receipt.get('package', '')).resolve():
        raise ValueError('candidate differs from exact Debian build artifact')
    source = receipt['source']
    provenance = receipt_path.parent / 'provenance'
    manifest = provenance / 'source-manifest.json'
    bundle = provenance / source['source_bundle_file']
    if digest(manifest) != source['manifest_sha256'] or digest(bundle) != source['source_bundle_sha256']:
        raise ValueError('candidate frozen source provenance changed')
    frozen = json.loads(manifest.read_text())
    if frozen['source_commit'] != source['source_commit'] or frozen['source_files'] != source['source_files']:
        raise ValueError('candidate source identity differs from build receipt')
    controls = subprocess.check_output(['ar', 'p', str(package), 'control.tar.xz'])
    with tarfile.open(fileobj=io.BytesIO(controls)) as archive:
        validate_control(archive.extractfile('./control').read().decode())
        preinst = archive.extractfile('./preinst').read()
    if hashlib.sha256(preinst).hexdigest() != source['source_files']['packaging/debian/preinst']['sha256']:
        raise ValueError('candidate account admission script differs from frozen source')
    raw = subprocess.check_output(['ar', 'p', str(package), 'data.tar.xz'])
    members = []
    identities = {}
    with tarfile.open(fileobj=io.BytesIO(raw)) as archive:
        for entry in archive.getmembers():
            name = entry.name.removeprefix('./')
            if entry.isdir():
                continue
            if not entry.isfile() or not name.startswith('usr/') or '..' in Path(name).parts or Path(name).is_absolute() or entry.size > 32 * 1024**2 or name in identities:
                raise ValueError('unsafe or duplicate candidate payload entry')
            content = archive.extractfile(entry).read()
            identities[name] = {'sha256': hashlib.sha256(content).hexdigest(), 'bytes': len(content)}
            members.append((name, content, 0o755 if name.startswith('usr/bin/') else 0o644))
    for required in ['usr/bin/fabric-node', 'usr/bin/fabric-server', 'usr/bin/fabricctl', 'usr/share/fabrico11y/console/asset-manifest.json', 'usr/share/doc/fabrico11y/THIRD-PARTY-NOTICES.json']:
        if required not in identities:
            raise ValueError('candidate payload incomplete: ' + required)
    if sum(len(body) for _, body, _ in members) > 128 * 1024**2:
        raise ValueError('candidate payload exceeds 128 MiB')
    destination.mkdir(parents=True, exist_ok=False)
    for name, content, mode in members:
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content)
        target.chmod(mode)
    (destination / 'candidate-preinst').write_bytes(preinst)
    record = {'role': 'RPM metadata around unchanged exact Debian12 candidate payload; no independent Fedora compilation claim',
              'deb_sha256': receipt['package_sha256'], 'build_receipt_sha256': digest(receipt_path),
              'source_commit': source['source_commit'], 'source_manifest_sha256': source['manifest_sha256'],
              'source_bundle_sha256': source['source_bundle_sha256'], 'source_date_epoch': source['source_date_epoch'],
              'adapter_sha256': digest(Path(__file__)),
              'rpm_builder_sha256': digest(Path(__file__).resolve().parents[2] / 'packaging/build-rpm.sh'),
              'payload_identity': identities}
    (destination / 'usr/share/doc/fabrico11y/CANDIDATE-PROVENANCE.json').write_text(json.dumps(record, indent=2, sort_keys=True) + '\n')
    return record


if __name__ == '__main__':
    require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('package', type=Path)
    parser.add_argument('receipt', type=Path)
    parser.add_argument('destination', type=Path)
    args = parser.parse_args()
    print(json.dumps(stage_candidate(args.package, args.receipt, args.destination), sort_keys=True))
