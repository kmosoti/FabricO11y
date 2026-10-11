"""Compare vendored crates with the crates.io archives pinned in vendor/provenance.json.

Cargo does not checksum path patches and cargo-deny does not inspect them.
"""
import hashlib, io, json, re, sys, tarfile, tomllib, urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def download(url: str) -> bytes:
    with urllib.request.urlopen(url, timeout=60) as response:
        return response.read(1 << 25)  # a longer archive fails its checksum


def vendored_files(path: Path) -> dict[str, bytes]:
    files = {}
    for item in sorted(path.rglob('*')):
        if item.is_symlink():
            raise ValueError(f'{item}: symbolic link')
        if item.is_file():
            files[item.relative_to(path).as_posix()] = item.read_bytes()
    return files


def patched_manifest(manifest: bytes, patch: dict) -> bytes:
    """The upstream manifest with only the declared dependency alias rewritten."""
    header = f'[dependencies.{patch["dependency_alias"]}]\n'
    new = header + f'package = "{patch["package"]}"\nversion = "{patch["version"]}"\n'
    return re.sub(re.escape(header) + r'version = "[^"\n]*"\n', lambda _: new,
                  manifest.decode(), count=1).encode()


def check_crate(root: Path, crate: dict, archive: bytes) -> list[str]:
    name = f'{crate["name"]}-{crate["version"]}'
    if hashlib.sha256(archive).hexdigest() != crate['archive_sha256']:
        return [f'{name}: archive SHA-256 differs from provenance']
    with tarfile.open(fileobj=io.BytesIO(archive), mode='r:gz') as tar:
        expected = {m.name[len(name) + 1:]: tar.extractfile(m).read() for m in tar if m.isfile()}
    local = vendored_files(root / crate['path'])
    for file in crate['modified']:
        expected[file] = patched_manifest(expected[file], crate['patch']) if file == 'Cargo.toml' else local.get(file)
    for file in crate['added']:
        expected[file] = local.get(file)
    return [f'{name}: {file}: ' + ('missing' if file not in local else 'not declared' if file not in expected
                                   else 'differs from upstream')
            for file in sorted(set(expected) | set(local))
            if local.get(file) is None or expected.get(file) is None or local[file] != expected[file]]


def check(root: Path, fetch=download) -> list[str]:
    crates = json.loads((root / 'vendor/provenance.json').read_text())['crates']
    patches = tomllib.loads((root / 'Cargo.toml').read_text())['patch']['crates-io']
    problems = []
    if {c['name']: c['path'] for c in crates} != {name: patch['path'] for name, patch in patches.items()}:
        problems.append('root [patch.crates-io] paths differ from vendor/provenance.json')
    for crate in crates:
        problems += check_crate(root, crate, fetch(crate['source']))
    return problems


if __name__ == '__main__':
    problems = check(ROOT)
    print(*problems or ['vendor: every crate matches its pinned archive apart from declared files'], sep='\n')
    sys.exit(1 if problems else 0)
