"""Prospective catalog cleanup: admit, byte-verify, then delete exact owned roots."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

import coupled_admit
from resource_group import STORAGE, require_limits

ROOT = Path(__file__).resolve().parents[4]
DATA = ROOT / 'docs/experiments/benchmarks/data/lab-completion-run-01'


def sha(stream):
    return hashlib.file_digest(stream, 'sha256').hexdigest()


def validate_archive(path, manifest):
    with tarfile.open(path, 'r:gz') as archive:
        members = archive.getmembers()
        if len(members) != len(manifest) or {m.name for m in members} != set(manifest):
            raise RuntimeError('archive coverage/duplicate mismatch')
        for member in members:
            wanted = manifest[member.name]
            if not member.isfile() or member.size != wanted['bytes']:
                raise RuntimeError('archive member type/size mismatch')
            with archive.extractfile(member) as stream:
                if sha(stream) != wanted['sha256']:
                    raise RuntimeError('archive byte digest mismatch')


def controls(work):
    manifest = {'file': {'bytes': 3, 'sha256': hashlib.sha256(b'abc').hexdigest()}}
    rejected = []
    for defect in ('valid', 'altered', 'duplicate', 'missing'):
        archive = work / (defect + '.tar.gz')
        with tarfile.open(archive, 'x:gz') as output:
            for _ in range(2 if defect == 'duplicate' else 0 if defect == 'missing' else 1):
                entry = tarfile.TarInfo('file')
                entry.size = 3
                output.addfile(entry, io.BytesIO(b'bad' if defect == 'altered' else b'abc'))
        try:
            validate_archive(archive, manifest)
        except RuntimeError:
            if defect == 'valid':
                raise
            rejected.append(defect)
        else:
            if defect != 'valid':
                raise RuntimeError('archive rejection control accepted: ' + defect)
    return rejected


def inactive(unit):
    result = subprocess.run(['systemctl', '--user', 'show', unit + '.service',
        '--property=ActiveState', '--property=LoadState'], capture_output=True, text=True)
    properties = dict(line.split('=', 1) for line in result.stdout.splitlines() if '=' in line)
    state = properties.get('ActiveState', '')
    if state not in ('inactive', 'failed') and properties.get('LoadState') != 'not-found':
        raise RuntimeError('service is active or its inactivity is unknown: ' + unit)
    return state or 'not-found'


def source_manifest(root):
    result = {}
    for path in root.rglob('*'):
        if path.is_symlink():
            raise RuntimeError('linked failure scratch requires separate review')
        if path.is_file():
            with path.open('rb') as stream:
                result[str(path.relative_to(root))] = {'bytes': path.stat().st_size, 'sha256': sha(stream)}
        elif not path.is_dir():
            raise RuntimeError('special failure file requires separate review')
    return result


def sync_copy(source, target):
    with source.open('rb') as incoming, target.open('xb') as outgoing:
        shutil.copyfileobj(incoming, outgoing)
        outgoing.flush()
        os.fsync(outgoing.fileno())


def main():
    require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--id', required=True)
    parser.add_argument('--only-job', action='append', required=True)
    args = parser.parse_args()
    ids = [args.id, *args.only_job]
    if any(not name.startswith('catalog-') or '/' in name or '\\' in name or name in ('.', '..') for name in ids):
        parser.error('exact catalog job IDs required')
    if len(set(args.only_job)) != len(args.only_job) or args.id in args.only_job:
        parser.error('duplicate/self cleanup job rejected')
    report_path = DATA / (args.id + '.json')
    if report_path.exists() or report_path.is_symlink():
        parser.error('fresh cleanup report required')
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True)
    if not scratch.is_relative_to((STORAGE / 'scratch').resolve()) or shutil.disk_usage(STORAGE).free < 16 * 1024**3:
        raise RuntimeError('owned data-drive scratch/reserve required')
    report = {'id': args.id, 'state': 'running', 'jobs': [], 'shared_build_sdk': 'preserved'}
    def record(event):
        report.setdefault('events', []).append(event)
        with report_path.open('w') as stream:
            json.dump(report, stream, indent=2)
            stream.write('\n')
            stream.flush()
            os.fsync(stream.fileno())
        print(json.dumps(event), flush=True)
    try:
        before = coupled_admit.observe(1024 * 1024, 0)  # Report/log and launcher-copy headroom.
        record({'before': before})
        with tempfile.TemporaryDirectory(prefix='catalog-cleanup-', dir=scratch) as temporary:
            work = Path(temporary)
            record({'rejected_archive_controls': controls(work)})
            for job in args.only_job:
                receipt_path = DATA / 'coordinator' / job / 'receipt.json'
                receipt = json.loads(receipt_path.read_text())
                if receipt['id'] != job or receipt['state'] not in ('failed', 'interrupted', 'passed'):
                    raise RuntimeError('exact completed job receipt required: ' + job)
                unit = receipt['cgroup'].rsplit('/', 1)[-1].removesuffix('.service')
                if not unit.startswith('fabric-work-') or '/' in unit:
                    raise RuntimeError('unexpected unit ownership')
                state = inactive(unit)
                original = ROOT / 'target/resource-containment/runs' / (unit + '.json')
                if original.is_symlink() or receipt_path.is_symlink():
                    raise RuntimeError('linked ownership receipt')
                outer = json.loads(original.read_text())
                retained_name = outer.get('retained_failure_evidence')
                row = {'id': job, 'state': state, 'original_exit': receipt.get('exit'), 'removed_bytes': 0}
                retained = Path(retained_name) if retained_name else None
                if retained is not None and (retained.parent != STORAGE / 'evidence' or retained.name != unit or retained.is_symlink()):
                    raise RuntimeError('failure root ownership mismatch')
                if receipt['lab'] not in ('memory', 'query', 'recovery', 'coordinator'):
                    raise RuntimeError('unexpected archive lab')
                archive = DATA / receipt['lab'] / ('failure-' + job + '.tar.gz')
                manifest_path = archive.with_suffix('.manifest.json')
                if archive.parent.is_symlink():
                    raise RuntimeError('linked archive lab directory')
                staged = None
                manifest = {}
                if retained is not None and retained.exists():
                    manifest = source_manifest(retained)
                    if archive.is_symlink() or manifest_path.is_symlink():
                        raise RuntimeError('linked archive destination')
                    if archive.exists():
                        validate_archive(archive, manifest)
                    else:
                        staged = work / (job + '.tar.gz')
                        with tarfile.open(staged, 'x:gz') as output:
                            for name in manifest:
                                output.add(retained / name, arcname=name, recursive=False)
                        validate_archive(staged, manifest)
                    if manifest_path.exists() and json.loads(manifest_path.read_text()) != manifest:
                        raise RuntimeError('existing archive manifest differs')
                manifest_bytes = (json.dumps(manifest, indent=2) + '\n').encode()
                # Stage on owned scratch; admit measured archive before ANY persistent
                # copy or deletion. Round lengths conservatively plus1MiB receipt margin.
                archive_reserve = (max(staged.stat().st_size, staged.stat().st_blocks * 512) + 4095) // 4096 * 4096 if staged else 0
                manifest_reserve = (len(manifest_bytes) + 4095) // 4096 * 4096 if retained is not None and retained.exists() and not manifest_path.exists() else 0
                reserve = archive_reserve + manifest_reserve + original.stat().st_size + 1024 * 1024
                admitted = coupled_admit.observe(reserve, archive_reserve + manifest_reserve if receipt['lab'] == 'memory' else 0)
                record({'job': job, 'archive_reserved_bytes': reserve, 'admitted': admitted})
                copies = DATA / 'coordinator/launcher-receipts'
                if copies.is_symlink():
                    raise RuntimeError('linked launcher receipt directory')
                copies.mkdir(exist_ok=True)
                copy = copies / (job + '.json')
                if copy.is_symlink():
                    raise RuntimeError('linked launcher receipt')
                if copy.exists():
                    if copy.read_bytes() != original.read_bytes():
                        raise RuntimeError('existing launcher receipt differs')
                else:
                    sync_copy(original, copy)
                if retained is not None and retained.exists():
                    if staged:
                        sync_copy(staged, archive)
                    validate_archive(archive, manifest)
                    if not manifest_path.exists():
                        with manifest_path.open('xb') as stream:
                            stream.write(manifest_bytes)
                            stream.flush()
                            os.fsync(stream.fileno())
                    # Rehash originals after archive creation; movement blocks deletion.
                    if source_manifest(retained) != manifest:
                        raise RuntimeError('failure source changed during archival')
                    record({'job': job, 'verified_archive': str(archive.relative_to(DATA)), 'ready_to_delete': True})
                    coupled_admit.observe(0, 0)  # Actual persistent allocation before deletion.
                    inactive(unit)
                    shutil.rmtree(retained)
                    row.update(removed_bytes=sum(v['bytes'] for v in manifest.values()),
                        archive=str(archive.relative_to(DATA)), byte_verified=True)
                row['scratch_removed'] = retained is None or not retained.exists()
                report['jobs'].append(row)
                record({'after_job': coupled_admit.observe(0, 0), 'job_result': row})
        report['state'] = 'completed'
        record({'after': coupled_admit.observe(0, 0), 'limitation': 'own final receipt/output complete after inventory'})
    except BaseException as error:
        report['state'] = 'failed'
        record({'error': repr(error), 'limitation': 'earlier jobs may already be archived and removed; consult events'})
        raise


if __name__ == '__main__':
    main()
