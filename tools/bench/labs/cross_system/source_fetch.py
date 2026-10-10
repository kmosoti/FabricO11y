#!/usr/bin/env python3
"""Retrieve immutable upstream source evidence without extraction or execution."""
import argparse
import fnmatch
import gzip
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import select
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits

MIB = 1024**2
COMPRESSED = 128 * MIB
DECODED = 2 * 1024**3
SELECTED = 16 * MIB
INVENTORY = 4 * MIB
EVIDENCE = 192 * MIB
RESERVE = 16 * 1024**3
CATALOG = Path(__file__).with_name('source_catalog.json')
REGISTERED = ROOT / 'docs/experiments/benchmarks/data/cross-system-run-01/source'


def dump(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(MIB), b''):
            h.update(chunk)
    return h.hexdigest()


def size(root):
    return sum(path.stat().st_size for path in root.rglob('*') if path.is_file())


def allocated_size(root):
    return sum(path.stat().st_blocks * 512 for path in root.rglob('*') if path.is_file())


def resolve(repo, pin, dest, deadline):
    if pin:
        if not isinstance(pin, str) or not re.fullmatch('[0-9a-f]{40}', pin):
            raise RuntimeError('registered pin is not an immutable40hex SHA')
        return pin, 'registered queue pin'
    argv = ['git', 'ls-remote', 'https://github.com/' + repo + '.git', 'HEAD']
    dump(dest / 'resolve-command.json', {'argv': argv})
    result = subprocess.run(argv, capture_output=True, timeout=max(1, min(30, deadline-time.monotonic())))
    (dest / 'resolve-stdout.txt').write_bytes(result.stdout)
    (dest / 'resolve-stderr.txt').write_bytes(result.stderr)
    if result.returncode:
        raise RuntimeError('git ls-remote exit ' + str(result.returncode))
    sha = result.stdout.decode().split()[0]
    if not re.fullmatch('[0-9a-f]{40}', sha):
        raise RuntimeError('HEAD resolution did not return an immutable SHA')
    return sha, 'git ls-remote HEAD at retrieval'


def download(repo, sha, archive, dest, deadline):
    remaining = max(1, int(deadline - time.monotonic()))
    argv = ['curl', '--fail', '--location', '--silent', '--show-error', '--connect-timeout', '15',
            '--max-time', str(remaining), '--max-filesize', str(COMPRESSED),
            'https://codeload.github.com/' + repo + '/tar.gz/' + sha]
    dump(dest / 'download-command.json', {'argv': argv})
    total = 0
    child = None
    try:
        with archive.open('wb') as output, (dest / 'download-stderr.txt').open('wb') as err:
            child = subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=err)
            while True:
                if time.monotonic() >= deadline:
                    raise RuntimeError('combined network deadline')
                ready, _, _ = select.select([child.stdout], [], [], min(1, max(0, deadline-time.monotonic())))
                if not ready:
                    continue
                chunk = os.read(child.stdout.fileno(), MIB)
                if not chunk:
                    break
                if total + len(chunk) > COMPRESSED:
                    # Preserve exactly the bounded prefix received; record truncation.
                    available = COMPRESSED-total
                    output.write(chunk[:available])
                    total += available
                    raise RuntimeError(f'{COMPRESSED}-byte compressed download ceiling; prefix preserved')
                output.write(chunk)
                total += len(chunk)
            code = child.wait(timeout=max(1, deadline-time.monotonic()))
            if code:
                raise RuntimeError('curl exit ' + str(code))
    finally:
        if child is not None and child.poll() is None:
            child.kill()
            child.wait()
        dump(dest / 'download.json', {'exit_code': child.returncode if child else None,
             'received_bytes': total, 'temporary_archive_sha256': digest(archive) if archive.exists() else None})


def relative_name(member):
    path = PurePosixPath(member.name)
    if path.is_absolute() or '..' in path.parts or len(path.parts) < 2:
        raise RuntimeError('unsafe archive member name: ' + member.name)
    return str(PurePosixPath(*path.parts[1:]))


class BoundedReader:
    """Count the entire decompressed tar stream, including headers and padding."""
    def __init__(self, stream, deadline):
        self.stream = stream
        self.decoded = 0
        self.deadline = deadline

    def read(self, count):
        if time.monotonic() > self.deadline:
            raise RuntimeError('1700-second source group deadline')
        block = self.stream.read(count)
        self.decoded += len(block)
        if self.decoded > DECODED:
            raise RuntimeError(f'{DECODED}-byte decompressed archive ceiling')
        return block


class BoundedWriter:
    def __init__(self, stream, limit):
        self.stream = stream
        self.limit = limit
        self.written = 0

    def write(self, block):
        if self.written + len(block) > self.limit:
            raise RuntimeError('compressed evidence artifact ceiling')
        count = self.stream.write(block)
        self.written += count
        return count

    def flush(self):
        self.stream.flush()

    def tell(self):
        return self.stream.tell()


def archive_readback(target, expected):
    """Compare every regular member exactly, rejecting duplicate/unsafe names."""
    checked = set()
    opener = (tarfile.open(fileobj=target, mode='r:gz') if hasattr(target, 'read')
              else tarfile.open(target, 'r:gz'))
    with opener as stream:
        for member in stream:
            path = PurePosixPath(member.name)
            if (not member.isfile() or path.is_absolute() or '..' in path.parts
                    or not path.parts or str(path) != member.name
                    or member.name in checked or member.name not in expected):
                raise RuntimeError('selected archive member type, duplicate or name mismatch')
            original = expected[member.name]
            original = original.read_bytes() if isinstance(original, Path) else original
            content = stream.extractfile(member).read()
            if content != original or member.size != len(original):
                raise RuntimeError('selected archive exact readback mismatch')
            checked.add(member.name)
    if checked != set(expected):
        raise RuntimeError('selected archive member-set mismatch')
    return checked


def census(archive, spec, dest, deadline):
    requested = set(spec['paths'])
    found = set()
    fallback = []
    licenses = []
    selected = {}
    selected_bytes = 0
    regular_bytes = 0
    regular_count = 0
    seen = set()
    inventory = dest / 'regular-members.jsonl.gz'
    with gzip.open(archive, 'rb') as compressed, inventory.open('wb') as inventory_output, \
            gzip.GzipFile(fileobj=BoundedWriter(inventory_output, INVENTORY), mode='wb') as inventory_gzip, \
            io.TextIOWrapper(inventory_gzip) as ledger:
        decoded = BoundedReader(compressed, deadline)
        with tarfile.open(fileobj=decoded, mode='r|') as stream:
            for member in stream:
                if not member.isfile():
                    continue
                name = relative_name(member)
                if name in seen:
                    raise RuntimeError('duplicate regular archive member: ' + name)
                seen.add(name)
                if member.size < 0 or regular_bytes + member.size > DECODED:
                    raise RuntimeError('2GiB regular member inventory ceiling')
                regular_bytes += member.size
                regular_count += 1
                license_file = PurePosixPath(name).name.upper().startswith(('LICENSE', 'COPYING', 'NOTICE'))
                source_name = PurePosixPath(name).suffix in {'.rs', '.cpp', '.h', '.hpp', '.go', '.md', '.txt'}
                is_fallback = name not in requested and source_name and len(fallback) < 12 and any(
                    fnmatch.fnmatchcase(name, pattern) for pattern in spec['fallback_patterns'])
                keep = name in requested or license_file or is_fallback
                if keep and selected_bytes + member.size > SELECTED:
                    raise RuntimeError('16MiB selected source/license ceiling')
                reader = stream.extractfile(member)
                h = hashlib.sha256()
                content = bytearray() if keep else None
                count = 0
                for chunk in iter(lambda: reader.read(MIB), b''):
                    count += len(chunk)
                    h.update(chunk)
                    if keep:
                        content.extend(chunk)
                if count != member.size:
                    raise RuntimeError('truncated archive member: ' + name)
                ledger.write(json.dumps({'path': name, 'bytes': count, 'sha256': h.hexdigest()}) + '\n')
                if keep:
                    selected[name] = (bytes(content), h.hexdigest())
                    selected_bytes += count
                if name in requested: found.add(name)
                if is_fallback: fallback.append(name)
                if license_file: licenses.append(name)
        # Drain after the end-of-tar markers: the ceiling covers trailing bytes too.
        while decoded.read(MIB):
            pass
    target = dest / 'selected-source.tar.gz'
    with target.open('wb') as selected_output:
        with tarfile.open(fileobj=BoundedWriter(selected_output, 20*MIB), mode='w:gz', compresslevel=1) as stream:
            for name, (content, _) in sorted(selected.items()):
                member = tarfile.TarInfo(name)
                member.size = len(content)
                member.mtime = 0
                stream.addfile(member, io.BytesIO(content))
    checked = archive_readback(target, {name: content for name, (content, _) in selected.items()})
    counted_members = counted_bytes = 0
    with gzip.open(inventory, 'rt') as ledger:
        for line in ledger:
            row = json.loads(line)
            counted_members += 1
            counted_bytes += row['bytes']
    if counted_members != regular_count or counted_bytes != regular_bytes:
        raise RuntimeError('inventory readback count mismatch')
    return {'regular_members': regular_count, 'regular_member_bytes': regular_bytes,
            'decompressed_tar_bytes': decoded.decoded, 'requested_paths': spec['paths'],
            'found_requested_paths': sorted(found), 'missing_requested_paths': sorted(requested-found),
            'fallback_paths': fallback, 'license_paths': licenses,
            'missing_license_files': not bool(licenses),
            'license_status': 'captured source files; no license opinion',
            'selected_source_bytes': selected_bytes, 'selected_archive_sha256': digest(target),
            'inventory_sha256': digest(inventory), 'inventory_readback_members': counted_members,
            'readback_verified_members': len(checked),
            'selected_members': {name: {'bytes': len(content), 'sha256': sha}
                                 for name, (content, sha) in selected.items()}}


def main():
    require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--destination', type=Path, required=True)
    parser.add_argument('--repos', nargs='+', required=True)
    parser.add_argument('--resolve-current', action='store_true',
                        help='Record a new HEAD pin after an unavailable historical revision')
    args = parser.parse_args()
    catalog = json.loads(CATALOG.read_text())
    specs = {entry['id']: entry for entry in catalog['repositories']}
    if any(name not in specs for name in args.repos):
        parser.error('unknown repository ID')
    if len(set(args.repos)) != len(args.repos):
        parser.error('duplicate repository ID')
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    destination = args.destination.resolve()
    data = Path('/run/media/kmosoti/data/FabricO11y').resolve()
    if not scratch.is_relative_to(data):
        parser.error('scratch must be on the required data drive')
    if not destination.is_relative_to(data) and destination != REGISTERED.resolve():
        parser.error('destination must be data-drive or registered source evidence root')
    destination.mkdir(parents=True, exist_ok=True)
    pins = json.loads((ROOT / 'docs/research/cross-system-lab-queue.json').read_text())['source_revisions']
    shutil.copyfile(CATALOG, destination / 'catalog.json')
    snapshot = destination / ('fetcher-' + digest(Path(__file__))[:16] + '.py')
    if not snapshot.exists():
        shutil.copyfile(Path(__file__), snapshot)
    deadline = time.monotonic() + 1700
    outcomes = []
    for name in args.repos:
        if time.monotonic() >= deadline:
            raise RuntimeError('1700-second group deadline')
        # Reserve the maximum complete failed download, plus bounded ledger overhead.
        if size(destination) + COMPRESSED + 25*MIB > EVIDENCE:
            raise RuntimeError('192MiB source allowance cannot preserve another worst-case download')
        if shutil.disk_usage(scratch).free < RESERVE:
            raise RuntimeError('16GiB free-space reserve')
        dest = destination / (name + '-current' if args.resolve_current else name)
        if dest.exists():
            raise RuntimeError('repository evidence already exists: ' + name)
        dest.mkdir()
        work = Path(tempfile.mkdtemp(prefix='source-' + name + '-', dir=scratch))
        archive = work / 'upstream.tar.gz'
        started = time.monotonic()
        receipt = {'id': name, 'repository': specs[name]['repo'], 'mechanism': specs[name]['mechanism'],
                   'catalog_sha256': digest(CATALOG), 'runner_sha256': digest(Path(__file__)),
                   'scratch': str(work), 'upstream_code_executed': False, 'source_audit': False,
                   'free_bytes_at_start': shutil.disk_usage(scratch).free,
                   'limits': {'compressed_download_bytes': COMPRESSED, 'decompressed_tar_bytes': DECODED,
                              'selected_source_bytes': SELECTED, 'retained_source_evidence_bytes': EVIDENCE,
                              'network_seconds': 120, 'group_seconds': 1700}}
        try:
            network_deadline = min(deadline, started + 120)
            sha, origin = resolve(specs[name]['repo'], None if args.resolve_current else pins.get(specs[name]['pin_key']), dest, network_deadline)
            receipt.update(revision=sha, revision_origin=origin,
                           immutable_url='https://codeload.github.com/' + specs[name]['repo'] + '/tar.gz/' + sha)
            dump(dest / 'revision.json', receipt)
            download(specs[name]['repo'], sha, archive, dest, network_deadline)
            receipt.update(archive_sha256=digest(archive), archive_bytes=archive.stat().st_size)
            receipt.update(census(archive, specs[name], dest, deadline))
            receipt.update(status='retrieved and selected-source readback verified', exit_code=0)
            dump(dest / 'receipt.json', receipt)
            archive.unlink()
        except BaseException as error:
            interrupted = not isinstance(error, Exception)
            receipt.update(status='retrieval interrupted' if interrupted else 'retrieval failed',
                           exit_code=130 if interrupted else 1, error=str(error))
            if archive.exists():
                preserved = dest / 'failed-download.tar.gz'
                shutil.copyfile(archive, preserved)
                receipt.update(failed_download_bytes=preserved.stat().st_size,
                               failed_download_sha256=digest(preserved))
                if digest(archive) != receipt['failed_download_sha256']:
                    raise RuntimeError('failed download preservation mismatch; scratch retained')
                archive.unlink()
            dump(dest / 'receipt.json', receipt)
            if interrupted:
                raise
        finally:
            receipt['wall_seconds'] = time.monotonic()-started
            if not any(work.iterdir()):
                work.rmdir()
            receipt['scratch_removed'] = not work.exists()
            receipt['retained_evidence_bytes'] = size(dest)
            receipt['retained_allocated_bytes'] = allocated_size(dest)
            receipt['free_bytes_after_cleanup'] = shutil.disk_usage(scratch).free
            dump(dest / 'receipt.json', receipt)
        outcomes.append({'id': name, 'status': receipt['status'], 'exit_code': receipt['exit_code']})
        if size(destination) > EVIDENCE:
            raise RuntimeError('192MiB retained source evidence ceiling; stop without deleting evidence')
    dump(destination / ('batch-' + args.repos[0] + ('-current' if args.resolve_current else '') + '.json'), {'outcomes': outcomes,
         'exit_code': 0 if all(row['exit_code'] == 0 for row in outcomes) else 1,
         'retained_evidence_bytes': size(destination)})
    if any(row['exit_code'] for row in outcomes):
        raise SystemExit(1)


if __name__ == '__main__':
    main()
