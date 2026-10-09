"""Preserve an inactive owned failed sweep tree, then remove its original.

Symlinks are archived as metadata, never followed for file contents or extracted.
The root coordinator must register the accompanying preservation supplement.
"""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import tarfile
import time

import prefix_load as shared
from run_sweep_job import CAPS

ROOT = shared.ROOT
MIB = 1024 ** 2
DATA = Path('/run/media/kmosoti/data/FabricO11y')
BASE = ROOT / 'docs/experiments/benchmarks/data/cross-system-sweep-01'
RAW_CAP = 512 * MIB
ARCHIVE_CAP = 192 * MIB


def check_deadline(deadline):
    if time.monotonic() >= deadline:
        raise TimeoutError('finite preservation deadline; retain original')


def checked_name(name):
    p = PurePosixPath(name)
    if not name or p.is_absolute() or any(c in ('', '.', '..') for c in name.split('/')):
        raise RuntimeError('archive traversal/noncanonical member')
    return name


def digest_stream(stream, deadline):
    digest, size = hashlib.sha256(), 0
    while block := stream.read(1024 * 1024):
        check_deadline(deadline)
        size += len(block)
        if size > RAW_CAP:
            raise RuntimeError('individual decoded member exceeds raw cap')
        digest.update(block)
    return size, digest.hexdigest()


def regular_input(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    if not stat.S_ISREG(os.fstat(descriptor).st_mode):
        os.close(descriptor)
        raise RuntimeError('inventoried regular input changed type')
    return os.fdopen(descriptor, 'rb')


def internal_link(root, name, target):
    if '\x00' in target:
        raise RuntimeError('NUL symlink target')
    resolved = ((root / name).parent / target).resolve(strict=True)
    if not resolved.is_relative_to(root):
        raise RuntimeError('symlink resolves outside owned root')
    return str(resolved.relative_to(root))


def inventory(root, deadline):
    members, total = {}, 0
    def visit(directory):
        nonlocal total
        for path in sorted(directory.iterdir()):
            check_deadline(deadline)
            name = checked_name(path.relative_to(root).as_posix())
            metadata = path.lstat()
            common = {'mode': stat.S_IMODE(metadata.st_mode), 'mtime_ns': metadata.st_mtime_ns}
            if stat.S_ISLNK(metadata.st_mode):
                target = os.readlink(path)
                members[name] = {**common, 'type': 'symlink', 'target': target,
                    'resolved_relative': internal_link(root, name, target)}
            elif stat.S_ISDIR(metadata.st_mode):
                members[name] = {**common, 'type': 'directory'}
                visit(path)
            elif stat.S_ISREG(metadata.st_mode):
                # Reading this explicit regular path never follows a link entry.
                with regular_input(path) as stream:
                    size, digest = digest_stream(stream, deadline)
                if size != metadata.st_size:
                    raise RuntimeError('owned regular file changed during inventory')
                total += size
                if total > RAW_CAP:
                    raise RuntimeError('whole owned raw exceeds512MiB; keep original')
                members[name] = {**common, 'type': 'regular', 'bytes': size, 'sha256': digest}
            else:
                raise RuntimeError('special file in owned failure; keep original')
    visit(root)
    return members, total


class LimitedOutput:
    def __init__(self, stream):
        self.stream, self.count = stream, 0

    def write(self, data):
        self.count += len(data)
        if self.count > ARCHIVE_CAP:
            raise RuntimeError('compressed archive exceeds128MiB; keep original')
        return self.stream.write(data)

    def flush(self):
        return self.stream.flush()


def write_archive(root, dest, members, deadline, *, omit=None, altered=None, bad_link=None):
    with dest.open('xb') as raw:
        bounded = LimitedOutput(raw)
        with tarfile.open(fileobj=bounded, mode='w|gz', format=tarfile.PAX_FORMAT) as archive:
            for name, entry in members.items():
                check_deadline(deadline)
                if name == omit:
                    continue
                info = tarfile.TarInfo(name)
                info.mode = entry['mode']
                info.mtime = entry['mtime_ns'] // 1_000_000_000
                # Exact nanosecond metadata is independently retained in manifest.
                if entry['type'] == 'directory':
                    info.type = tarfile.DIRTYPE
                    archive.addfile(info)
                elif entry['type'] == 'symlink':
                    info.type = tarfile.SYMTYPE
                    info.linkname = bad_link if name == 'link' and bad_link is not None else entry['target']
                    archive.addfile(info)
                else:
                    info.type, info.size = tarfile.REGTYPE, entry['bytes']
                    if name == altered:
                        import io
                        archive.addfile(info, io.BytesIO(b'bad'))
                    else:
                        # Manual addfile prevents tarfile's hardlink deduplication.
                        with regular_input(root / name) as stream:
                            archive.addfile(info, stream)


def verify_archive(root, archive_path, members, deadline):
    seen, total = set(), 0
    if archive_path.stat().st_size > ARCHIVE_CAP:
        raise RuntimeError('archive over128MiB')
    with tarfile.open(archive_path, 'r|gz') as archive:
        for member in archive:
            check_deadline(deadline)
            name = checked_name(member.name.rstrip('/') if member.isdir() else member.name)
            if name in seen or name not in members:
                raise RuntimeError('duplicate/uninventoried archive member')
            seen.add(name)
            expected = members[name]
            if member.mode != expected['mode'] or member.mtime != expected['mtime_ns'] // 1_000_000_000:
                raise RuntimeError('archive metadata mismatch')
            if expected['type'] == 'regular':
                if not member.isfile() or member.islnk() or member.size != expected['bytes']:
                    raise RuntimeError('regular member type/size mismatch')
                with archive.extractfile(member) as stream:
                    size, digest = digest_stream(stream, deadline)
                if size != expected['bytes'] or digest != expected['sha256']:
                    raise RuntimeError('regular member exact-byte hash mismatch')
                total += size
                if total > RAW_CAP:
                    raise RuntimeError('decoded archive exceeds512MiB')
            elif expected['type'] == 'directory':
                if not member.isdir() or member.size != 0:
                    raise RuntimeError('directory member mismatch')
            else:
                if not member.issym() or member.size != 0 or member.linkname != expected['target']:
                    raise RuntimeError('symlink metadata mismatch')
                if internal_link(root, name, member.linkname) != expected['resolved_relative']:
                    raise RuntimeError('symlink resolved target mismatch')
    if seen != set(members):
        raise RuntimeError('archive missing owned member')
    return {'members': len(seen), 'regular_bytes': total, 'exact_manifest_readback': True,
            'symlink_contents_followed': False, 'archive_extracted': False}


def negative_controls(scratch, deadline):
    work = scratch / 'preservation-controls'
    work.mkdir()
    (work / 'payload').write_bytes(b'abc')
    (work / 'link').symlink_to('payload')
    members, _ = inventory(work, deadline)
    archive = scratch / 'valid-control.tar.gz'
    write_archive(work, archive, members, deadline)
    verify_archive(work, archive, members, deadline)
    rejected = []
    for name, kwargs in [('mutated-payload', {'altered': 'payload'}),
                         ('missing-file', {'omit': 'payload'}),
                         ('bad-link-target', {'bad_link': '../../outside-owned-root'})]:
        dest = scratch / (name + '.tar.gz')
        write_archive(work, dest, members, deadline, **kwargs)
        try:
            verify_archive(work, dest, members, deadline)
        except (RuntimeError, FileNotFoundError):
            rejected.append(name)
        else:
            raise RuntimeError('archive checker accepted injected ' + name)
    # Separately exercise the ownership boundary, even if an attacker also
    # supplies a matching bad target string in a forged manifest.
    try:
        internal_link(work, 'link', str(DATA))
    except RuntimeError:
        rejected.append('outside-target-boundary')
    else:
        raise RuntimeError('internal target checker accepted outside root')
    for path in scratch.glob('*control.tar.gz'):
        path.unlink()
    for name in ('mutated-payload', 'missing-file', 'bad-link-target'):
        (scratch / (name + '.tar.gz')).unlink()
    shutil.rmtree(work)
    return rejected


def main():
    if os.environ.get('FABRIC_CROSS_SYSTEM_COORDINATED') != '1':
        raise RuntimeError('root coordinator ownership required')
    shared.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--category', choices=('query', 'source'), required=True)
    parser.add_argument('--origin', type=Path, action='append', required=True)
    parser.add_argument('--proposal', type=Path, required=True)
    parser.add_argument('--empty-only', action='store_true')
    args = parser.parse_args()
    root = args.root.absolute()
    if (root.parent != DATA / 'evidence' or not re.fullmatch(r'fabric-work-[0-9a-f]{32}', root.name)
            or root.is_symlink() or root.resolve(strict=True) != root or not root.is_dir()):
        raise RuntimeError('only original owned data/evidence/fabric-work-* root is admitted')
    out = args.out.absolute()
    category = BASE / args.category
    if (not out.is_relative_to(category) or out.exists() or out.resolve() != out
            or out.parent.resolve() != out.parent):
        raise RuntimeError('fresh unlinked category evidence destination required')
    cap = CAPS[args.category] * MIB
    if shared.footprint(category) + ARCHIVE_CAP + MIB > cap:
        raise RuntimeError('category cannot reserve complete bounded failure archive plus metadata')
    if not args.proposal.is_file() or any(not p.is_file() for p in args.origin):
        raise RuntimeError('registered proposal/origin unavailable')
    out.mkdir(parents=True)
    started = time.monotonic()
    deadline = started + 240
    record = {'state': 'running', 'root': str(root), 'origin': [str(p) for p in args.origin],
        'origin_sha256': {str(p): shared.sha(p) for p in args.origin},
        'helper_sha256': shared.sha(Path(__file__)), 'proposal_sha256': shared.sha(args.proposal),
        'raw_cap_bytes': RAW_CAP, 'archive_cap_bytes': ARCHIVE_CAP,
        'category_bytes_before': shared.footprint(category), 'inactive_root': 'owner-confirmed before admission',
        'empty_only': args.empty_only, 'original_removed': False, 'checks': []}
    for i, source in enumerate([Path(__file__).resolve(), args.proposal, *args.origin]):
        payload = source.read_bytes()
        dest = out / f'origin-source-{i:02}.gz'
        dest.write_bytes(gzip.compress(payload, mtime=0))
        if gzip.decompress(dest.read_bytes()) != payload:
            raise RuntimeError('origin/source snapshot readback mismatch; retain original')
    shared.dump(out / 'receipt.json', record)
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True) / 'preserve-controls'
    scratch.mkdir()
    try:
        record['negative_controls'] = negative_controls(scratch, deadline)
        scratch.rmdir()
        manifest, total = inventory(root, deadline)
        if args.empty_only and manifest:
            raise RuntimeError('empty-only cleanup found owned entries; retain original')
        shared.dump(out / 'manifest.json', {'members': manifest, 'regular_bytes': total,
            'root_mode': stat.S_IMODE(root.stat().st_mode)})
        archive = out / 'whole-owned-tree.tar.gz'
        write_archive(root, archive, manifest, deadline)
        record['archive_sha256'] = shared.sha(archive)
        for _ in range(2):
            record['checks'].append(verify_archive(root, archive, manifest, deadline))
        # Recheck original metadata, content hashes and links after both reads;
        # cleanup is authorized only for the very tree that was preserved.
        current, current_total = inventory(root, deadline)
        if current != manifest or current_total != total or shared.sha(archive) != record['archive_sha256']:
            raise RuntimeError('owned tree/archive changed before cleanup; retain original')
        if shared.footprint(category) > cap:
            raise RuntimeError('retained category cap exceeded; retain original')
        record.update(state='verified', regular_bytes=total)
        shared.dump(out / 'receipt.json', record)
        check_deadline(deadline)
        shutil.rmtree(root)
        record.update(state='complete', original_removed=not root.exists())
    except BaseException as error:
        record.update(state='failed', error=repr(error), original_removed=not root.exists())
        raise
    finally:
        record.update(elapsed_s=time.monotonic() - started, category_bytes_after=shared.footprint(category),
                      control_scratch_retained=scratch.exists())
        shared.dump(out / 'receipt.json', record)


if __name__ == '__main__':
    main()
