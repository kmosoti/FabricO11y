"""Preserve exact already-built storage inputs and workspace build metadata."""
import gzip
import io
import json
from pathlib import Path
import subprocess
import tarfile

from source_fetch import ROOT, archive_readback, digest, dump, require_limits


def main():
    require_limits()
    base = ROOT / 'docs/experiments/benchmarks/data/cross-system-sweep-01'
    built = json.loads((base / 'memory/builds/build-manifest.json').read_text())
    expected = {}
    for name, recorded in built['source_sha256'].items():
        path = ROOT / name
        if digest(path) != recorded:
            raise RuntimeError('source differs from frozen build: ' + name)
        expected[name] = path.read_bytes()
    for path in [ROOT / 'Cargo.toml', *sorted((ROOT / 'crates').glob('*/Cargo.toml')),
                 ROOT / '.cargo/config.toml']:
        if path.exists():
            expected[str(path.relative_to(ROOT))] = path.read_bytes()
    if sum(map(len, expected.values())) > 16 * 1024**2:
        raise RuntimeError('source snapshot ceiling')
    target = base / 'coordinator/storage-source-snapshot.tar.gz'
    with target.open('xb') as output, tarfile.open(fileobj=output, mode='w:gz') as archive:
        for name, payload in sorted(expected.items()):
            member = tarfile.TarInfo(name)
            member.size = len(payload)
            archive.addfile(member, io.BytesIO(payload))
    archive_readback(target, expected)
    diff = subprocess.check_output(['git', 'diff', '--binary', 'HEAD'], cwd=ROOT)
    (base / 'coordinator/storage-tracked-diff.gz').write_bytes(gzip.compress(diff, mtime=0))
    dump(base / 'coordinator/storage-source-snapshot.json', dict(
        archive_sha256=digest(target), exact_readback_members=len(expected),
        build_source_hashes_matched=True,
        rustc=subprocess.check_output(['rustc', '-Vv'], text=True),
        cargo=subprocess.check_output(['cargo', '-V'], text=True),
        revision=subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
        additional_build_metadata='Captured after build; no manifest edits occurred between build and capture.'))


if __name__ == '__main__':
    main()
