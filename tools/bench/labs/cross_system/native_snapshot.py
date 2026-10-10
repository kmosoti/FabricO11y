"""Freeze exact already-built native memory sources, including untracked inputs.

Based on freeze_sweep_sources.py; no build, source edit, extraction or deletion.
"""
import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import subprocess
import tarfile

from run_native_job import ROOT, dump, require_limits, sha

DECODED_CAP = 16 * 1024**2


def digest(payload):
    return hashlib.sha256(payload).hexdigest()


def regular_source(name):
    parts = PurePosixPath(name)
    if (parts.is_absolute() or str(parts) != name or not name
            or any(p in ('', '.', '..') for p in parts.parts)):
        raise RuntimeError('noncanonical snapshot source path')
    path = ROOT / name
    if not path.is_file() or any(p.is_symlink() for p in [path, *path.parents] if p != ROOT.parent):
        raise RuntimeError('nonregular/linked snapshot source: ' + name)
    return path


def bounded_command(argv, cap):
    """Drain known metadata commands without holding an unbounded stdout buffer."""
    with subprocess.Popen(argv, cwd=ROOT, stdout=subprocess.PIPE) as child:
        output = bytearray()
        while True:
            block = child.stdout.read(min(65536, cap-len(output)+1))
            if not block:
                break
            output.extend(block)
            if len(output) > cap:
                child.kill()
                child.wait()
                raise RuntimeError('snapshot metadata exceeds decoded ceiling')
        result = child.wait(timeout=30)
        if result:
            raise RuntimeError('snapshot metadata command failed: ' + repr(argv))
    return bytes(output)


def archive_bytes(entries):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode='w:gz') as archive:
        for name, payload in sorted(entries.items()):
            member = tarfile.TarInfo(name)
            member.size = len(payload)
            archive.addfile(member, io.BytesIO(payload))
    return output.getvalue()


def verify_archive(stream, expected):
    seen = set()
    total = 0
    with tarfile.open(fileobj=stream, mode='r|gz') as archive:
        for member in archive:
            if member.name in seen or member.name not in expected or not member.isfile():
                raise RuntimeError('duplicate/unlisted/nonregular snapshot member')
            seen.add(member.name)
            spec = expected[member.name]
            total += member.size
            if total > DECODED_CAP or member.size != spec['bytes']:
                raise RuntimeError('snapshot member size/ceiling differs')
            hashed = hashlib.sha256()
            size = 0
            with archive.extractfile(member) as payload:
                while block := payload.read(65536):
                    hashed.update(block)
                    size += len(block)
            if size != spec['bytes'] or hashed.hexdigest() != spec['sha256']:
                raise RuntimeError('snapshot member bytes differ')
    if seen != set(expected):
        raise RuntimeError('missing snapshot member')
    return {'exact_readback_members': len(seen), 'decoded_bytes': total, 'extracted': False}


def readback_controls():
    fixture = {'fixture.rs': b'original'}
    expected = {k: {'bytes': len(v), 'sha256': digest(v)} for k, v in fixture.items()}
    verify_archive(io.BytesIO(archive_bytes(fixture)), expected)
    rejected = []
    for label, entries in [('changed_same_size', {'fixture.rs': b'modified'}),
                           ('missing', {}), ('unlisted', {'other.rs': b'original'})]:
        try:
            verify_archive(io.BytesIO(archive_bytes(entries)), expected)
        except RuntimeError:
            rejected.append(label)
        else:
            raise RuntimeError('snapshot defect control accepted: ' + label)
    return {'positive_exact_readback': True, 'rejected': rejected,
            'scope': 'small in-memory archives, same readback checker as real capture'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--build-manifest', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    require_limits()
    if args.out.exists():
        raise RuntimeError('fresh source snapshot destination required')
    built = json.loads(args.build_manifest.read_text())
    sources = built['source_sha256']
    if not sources:
        raise RuntimeError('empty frozen build source map')
    entries = {}
    frozen_paths = {}
    for name, recorded in sources.items():
        path = regular_source(name)
        if sum(map(len, entries.values())) + path.stat().st_size > DECODED_CAP:
            raise RuntimeError('build sources exceed decoded snapshot ceiling')
        if sha(path) != recorded:
            raise RuntimeError('source differs from frozen build: ' + name)
        payload = path.read_bytes()
        if digest(payload) != recorded:
            raise RuntimeError('source changed while capturing: ' + name)
        entries[name] = payload
        frozen_paths[name] = path
    for path in [ROOT/'Cargo.toml', *sorted((ROOT/'crates').glob('*/Cargo.toml')), ROOT/'.cargo/config.toml']:
        if path.exists():
            name = str(path.relative_to(ROOT))
            path = regular_source(name)
            if name not in entries:
                if sum(map(len, entries.values())) + path.stat().st_size > DECODED_CAP:
                    raise RuntimeError('workspace metadata exceeds snapshot ceiling')
                entries[name] = path.read_bytes()
    commands = {
        'revision.txt': ['git', 'rev-parse', 'HEAD'],
        'rustc-version.txt': ['rustc', '-Vv'],
        'cargo-version.txt': ['cargo', '-V'],
        'cargo-metadata.json': ['cargo', 'metadata', '--offline', '--locked', '--no-deps', '--format-version', '1'],
        'working-diff.bin': ['git', 'diff', '--binary', 'HEAD'],
        'working-status.txt': ['git', 'status', '--short', '--untracked-files=all'],
    }
    metadata = {}
    for name, command in commands.items():
        payload = bounded_command(command, DECODED_CAP-sum(map(len, entries.values())))
        archive_name = '_native_snapshot/' + name
        if archive_name in entries:
            raise RuntimeError('snapshot metadata/source name collision')
        entries[archive_name] = payload
        metadata[name] = payload
    expected = {k: {'bytes': len(v), 'sha256': digest(v)} for k, v in entries.items()}
    args.out.mkdir(parents=True)
    archive = args.out/'exact-build-sources.tar.gz'
    with archive.open('xb') as output, tarfile.open(fileobj=output, mode='w:gz') as packed:
        for name, payload in sorted(entries.items()):
            member = tarfile.TarInfo(name)
            member.size = len(payload)
            packed.addfile(member, io.BytesIO(payload))
    with archive.open('rb') as stream:
        readback = verify_archive(stream, expected)
    for name, path in frozen_paths.items():
        if sha(path) != sources[name]:
            raise RuntimeError('build source changed during snapshot: ' + name)
    (args.out/'working-diff.bin.gz').write_bytes(gzip.compress(metadata['working-diff.bin'], mtime=0))
    dump(args.out/'members.json', expected)
    dump(args.out/'receipt.json', {'state': 'complete', 'helper_sha256': sha(Path(__file__)),
        'build_manifest_path': str(args.build_manifest), 'build_manifest_sha256': sha(args.build_manifest),
        'archive_sha256': sha(archive), 'archive_bytes': archive.stat().st_size,
        'members_sha256': sha(args.out/'members.json'), **readback,
        'build_source_hashes_matched_before_and_after_capture': True,
        'working_diff_sha256': digest(metadata['working-diff.bin']),
        'working_diff_gzip_sha256': sha(args.out/'working-diff.bin.gz'),
        'revision': metadata['revision.txt'].decode().strip(),
        'rustc': metadata['rustc-version.txt'].decode(), 'cargo': metadata['cargo-version.txt'].decode(),
        'metadata_commands': commands, 'rejected_controls': readback_controls(),
        'limits': '16MiB combined decoded source and metadata ceiling',
        'scope': 'exact build-manifest sources include dirty/untracked files; tracked diff and status also retained; unlisted untracked bytes not claimed captured',
        'build_metadata_limit': 'workspace manifests/metadata captured after build; frozen build source hashes authenticate listed files only'})


if __name__ == '__main__':
    main()
