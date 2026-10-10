"""Bundle dependency license texts from the exact locked whole-workspace graph."""
import argparse
import hashlib
import gzip
import tarfile
import os
import tomllib
import json
from pathlib import Path
import shutil
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from resource_group import require_limits

ROOT = Path(__file__).resolve().parents[2]
MPL_CRATES = {'base64urlsafedata', 'webauthn-attestation-ca', 'webauthn-rs', 'webauthn-rs-core', 'webauthn-rs-proto'}


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def supplemental_texts(package):
    manifest = json.loads((ROOT / 'packaging/licenses/upstream-manifest.json').read_text())
    records = [r for r in manifest if r.get('crate') == package['name'] and r.get('version') == package['version']]
    if len(records) != 1:
        raise ValueError('missing exact upstream license provenance: ' + package['id'])
    record = records[0]
    paths = set()
    for entry in record['texts']:
        name = entry['file']
        if Path(name).name != name:
            raise ValueError('invalid supplemental license path')
        path = ROOT / 'packaging/licenses' / (package['name'] + '-' + package['version']) / name
        if path.is_symlink() or digest(path) != entry['sha256']:
            raise ValueError('changed upstream dependency license: ' + str(path))
        paths.add(path)
    return paths, record


def bundle_locked_source(package, destination):
    if package['source'] is None:
        directory = Path(package['manifest_path']).parent
        if not directory.is_relative_to(ROOT / 'vendor'):
            raise ValueError('unregistered local dependency source: ' + package['id'])
        target = destination / 'licenses/source' / (package['name'] + '-' + package['version'] + '-local.tar.gz')
        target.parent.mkdir(parents=True, exist_ok=True)
        files = sorted(p for p in directory.rglob('*') if p.is_symlink() or not p.is_dir())
        if any(p.is_symlink() or not p.is_file() for p in files) or sum(p.stat().st_size for p in files) > 32 * 1024**2:
            raise ValueError('invalid or oversized local dependency source')
        with target.open('wb') as output, gzip.GzipFile(filename='', mode='wb', fileobj=output, mtime=0) as compressed, tarfile.open(fileobj=compressed, mode='w') as archive:
            for path in files:
                info = archive.gettarinfo(str(path), arcname=package['name'] + '-' + package['version'] + '/' + path.relative_to(directory).as_posix())
                info.uid = info.gid = info.mtime = 0
                info.uname = info.gname = ''
                with path.open('rb') as stream:
                    archive.addfile(info, stream)
        target.chmod(0o644)
        return {'path': str(target.relative_to(destination)), 'sha256': digest(target), 'bytes': target.stat().st_size,
                'format': 'local patched source; includes exact modified manifest and upstream files'}
    locked = tomllib.loads((ROOT / 'Cargo.lock').read_text())['package']
    matches = [p for p in locked if p['name'] == package['name'] and p['version'] == package['version'] and p.get('source') == package['source']]
    if len(matches) != 1 or 'checksum' not in matches[0]:
        raise ValueError('missing locked source checksum: ' + package['id'])
    home = Path(os.environ['CARGO_HOME'])
    filename = package['name'] + '-' + package['version'] + '.crate'
    archives = list((home / 'registry/cache').glob('*/' + filename))
    if len(archives) != 1 or archives[0].is_symlink() or digest(archives[0]) != matches[0]['checksum']:
        raise ValueError('missing or changed locked source: ' + filename)
    target = destination / 'licenses/source' / filename
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(archives[0], target)
    target.chmod(0o644)
    return {'path': str(target.relative_to(destination)), 'sha256': matches[0]['checksum'], 'bytes': target.stat().st_size,
            'format': 'unchanged crates.io crate source archive'}



def bundle_mpl_source(package, destination):
    if package['name'] not in MPL_CRATES or package['version'] != '0.5.5':
        raise ValueError('MPL source not covered by exact policy: ' + package['id'])
    return bundle_locked_source(package, destination)


def generate(destination):
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--offline', '--locked', '--all-features', '--format-version=1'], cwd=ROOT))
    workspace = set(metadata['workspace_members'])
    resolved = {node['id'] for node in metadata['resolve']['nodes']}
    packages = sorted((p for p in metadata['packages'] if p['id'] in resolved and p['id'] not in workspace),
                      key=lambda p: (p['name'], p['version'], p['id']))
    notices = []
    total = 0
    for package in packages:
        directory = Path(package['manifest_path']).parent
        texts = {p for p in directory.iterdir() if p.is_file() and p.name.upper().startswith(('LICENSE', 'COPYING', 'NOTICE'))}
        # r-efi keeps its complete permission/copyright notice in AUTHORS.
        authors = directory / 'AUTHORS'
        if authors.is_file() and 'AUTHORS-MIT:' in authors.read_text():
            texts.add(authors)
        if package.get('license_file'):
            texts.add(directory / package['license_file'])
        provenance = None
        if not texts:
            texts, provenance = supplemental_texts(package)
        names = []
        target = destination / 'licenses' / (package['name'] + '-' + package['version'])
        target.mkdir(parents=True, exist_ok=False)
        for text in sorted(texts):
            if not text.is_file() or text.stat().st_size > 1024 * 1024:
                raise ValueError('invalid or oversized dependency license: ' + str(text))
            total += text.stat().st_size
            if total > 32 * 1024 * 1024:
                raise ValueError('dependency license bundle exceeds 32 MiB')
            shutil.copyfile(text, target / text.name)
            (target / text.name).chmod(0o644)
            names.append(str((target / text.name).relative_to(destination)))
        notice = {'crate': package['name'], 'version': package['version'],
                  'source': package['source'], 'declared_license': package['license'],
                  'bundled_texts': names}
        if provenance:
            notice['upstream_license_provenance'] = provenance
        if provenance and provenance.get('kind') == 'documentary':
            notice['local_source'] = bundle_locked_source(package, destination)
        if 'MPL-2.0' in (package.get('license') or ''):
            notice['local_source'] = bundle_mpl_source(package, destination)
        notices.append(notice)
    sources = [dict(crate=n['crate'], version=n['version'], **n['local_source']) for n in notices if 'local_source' in n]
    source_dir = destination / 'licenses/source'
    source_dir.mkdir(parents=True, exist_ok=True)
    (source_dir / 'manifest.json').write_text(json.dumps(sources, indent=2, sort_keys=True) + '\n')
    (destination / 'THIRD-PARTY-NOTICES.json').write_text(json.dumps(notices, indent=2, sort_keys=True) + '\n')
    return len(notices)


if __name__ == '__main__':
    require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('destination', type=Path)
    args = parser.parse_args()
    print('bundled dependency notices:', generate(args.destination))
