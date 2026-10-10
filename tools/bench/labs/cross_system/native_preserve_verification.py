"""Preserve one of two exact inactive fast-check failures before owned-tree removal.

Reuses sweep_preserve's unchanged inventory/archive/readback/control functions.
"""
import argparse
import gzip
import hashlib
import os
from pathlib import Path
import shutil
import stat
import subprocess
import time

import sweep_preserve as preserve
from run_native_job import BASE, CAPS, ROOT, STORAGE, dump, footprint, require_limits, sha

FAILED_JOBS = {
    'final-fast-01': ('fabric-work-c3a63f9ab40a477c9f3189eb298aee66',
                      'native-verification-retry-proposal.md'),
    'final-fast-02': ('fabric-work-21ec85a38b874063863ef6327431e027',
                      'native-verification-example-retry-proposal.md'),
}
RAW_CAP = 64 * 1024**2
ARCHIVE_CAP = 8 * 1024**2


def regular(path):
    if path.is_symlink() or not path.is_file() or path.resolve(strict=True) != path:
        raise RuntimeError('origin/helper path is linked or nonregular')


def selected_failure(job):
    if job not in FAILED_JOBS:
        raise RuntimeError('failed job outside exact two-job allowlist')
    return FAILED_JOBS[job]


def inactive_unit(unit):
    result = subprocess.run(['systemctl', '--user', 'is-active', unit], capture_output=True,
                            text=True, timeout=5)
    state = result.stdout.strip()
    if result.returncode not in (3, 4) or state not in ('inactive', 'failed', 'unknown'):
        raise RuntimeError('failed unit is not independently inactive')
    current = Path('/sys/fs/cgroup') / next(line[3:] for line in Path('/proc/self/cgroup').read_text().splitlines()
        if line.startswith('0::')).lstrip('/')
    group = current.parent/(unit+'.service')
    if group.exists():
        if any(path.read_text().strip() for path in [group/'cgroup.procs', *group.glob('**/cgroup.procs')]):
            raise RuntimeError('failed unit still has processes')
    return {'unit': unit, 'is_active_exit': result.returncode, 'state': state,
            'stderr': result.stderr.strip(), 'inspected_cgroup': str(group),
            'cgroup_exists': group.exists(), 'failed_unit_processes_absent': True}


def main():
    import json
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--failed-job', choices=tuple(FAILED_JOBS), default='final-fast-01')
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--proposal', type=Path, required=True)
    args = parser.parse_args()
    require_limits()
    if os.environ.get('FABRIC_NATIVE_COORDINATED') != '1':
        raise RuntimeError('inherited native coordinator ownership required')
    started = time.monotonic()
    deadline = started + 50  # leave bounded finalization time within a 60-second job
    # These configure the unchanged functions in this isolated helper process;
    # they do not mutate sweep_preserve.py or historical receipts.
    preserve.RAW_CAP = RAW_CAP
    preserve.ARCHIVE_CAP = ARCHIVE_CAP
    unit, proposal_name = selected_failure(args.failed_job)
    root = STORAGE/'evidence'/unit
    out = args.out.absolute()
    job_dir = BASE/'coordinator'/Path(os.environ['FABRIC_SCRATCH_ROOT']).name
    if (out != job_dir/'preservation' or out.exists() or out.resolve() != out
            or out.parent.resolve() != out.parent):
        raise RuntimeError('fresh current-job nested preservation destination required')
    proposal = args.proposal.absolute()
    if proposal != ROOT/'docs/experiments/benchmarks'/proposal_name:
        raise RuntimeError('wrong prospective verification retry proposal')
    outer_path = ROOT/'target/resource-containment/runs'/(unit+'.json')
    native_path = BASE/'coordinator'/args.failed_job/'receipt.json'
    failed_runner = native_path.parent/'runner.py.gz'
    sources = [Path(__file__).resolve(), Path(preserve.__file__).resolve(),
               Path(preserve.shared.__file__).resolve(), Path(__file__).with_name('run_native_job.py'),
               Path(__file__).with_name('run_sweep_job.py'), ROOT/'tools/resource_group.py',
               ROOT/'docs/experiments/benchmarks/native-verification-retry-proposal.md',
               ROOT/'docs/experiments/benchmarks/native-verification-example-retry-proposal.md',
               outer_path, native_path, failed_runner,
               native_path.parent/'checks/clippy.json']
    for path in sources:
        regular(path)
    outer = json.loads(outer_path.read_text())
    native = json.loads(native_path.read_text())
    if hashlib.sha256(gzip.decompress(failed_runner.read_bytes())).hexdigest() != native['runner_sha256']:
        raise RuntimeError('failed job frozen runner snapshot differs')
    argv = outer['command']
    if (outer.get('unit') != unit or outer.get('exit') != 1
            or outer.get('retained_failure_evidence') != str(root)
            or not outer.get('temporary_removed') or argv.count('--id') != 1
            or argv[argv.index('--id')+1] != args.failed_job
            or native.get('id') != args.failed_job or native.get('state') != 'failed'
            or native.get('exit') != 1 or native.get('child_exit') != 1):
        raise RuntimeError('exact failed unit/job/origin binding differs')
    if root.is_symlink() or not root.is_dir() or root.resolve(strict=True) != root:
        raise RuntimeError('exact original retained failed unit tree unavailable')
    inactive = inactive_unit(unit)
    category = BASE/'coordinator'
    before = footprint(category)
    if before + ARCHIVE_CAP + 1024**2 > CAPS['coordinator']*1024**2:
        raise RuntimeError('coordinator lacks complete archive/metadata reserve')
    out.mkdir()
    record = {'state': 'running', 'root': str(root), 'original_removed': False,
        'failed_job': args.failed_job, 'inactive_unit': inactive, 'raw_cap_bytes': RAW_CAP,
        'archive_cap_bytes': ARCHIVE_CAP, 'deadline_seconds': 50,
        'helper_sha256': sha(Path(__file__)), 'proposal_sha256': sha(proposal),
        'origin_sha256': {str(p): sha(p) for p in (outer_path, native_path)},
        'source_sha256': {str(p): sha(p) for p in sources}, 'category_bytes_before': before,
        'checks': [], 'scope': 'exact failed verification tree; symbolic links metadata only, no extraction'}
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT'])/'verification-preservation-controls'
    dump(out/'receipt.json', record)
    try:
        for i, source in enumerate(sources):
            payload = source.read_bytes()
            if len(payload) > 1024**2 or sha(source) != record['source_sha256'][str(source)]:
                raise RuntimeError('origin/source changed or exceeded snapshot bound')
            dest = out/f'origin-source-{i:02}.gz'
            dest.write_bytes(gzip.compress(payload, mtime=0))
            if gzip.decompress(dest.read_bytes()) != payload:
                raise RuntimeError('source snapshot exact readback differs')
        scratch.mkdir()
        record['negative_controls'] = preserve.negative_controls(scratch, deadline)
        try:
            selected_failure('final-fast-03')
        except RuntimeError:
            record['negative_controls'].append('unknown-failed-job')
        else:
            raise RuntimeError('unknown failed job control accepted')
        scratch.rmdir()
        members, total = preserve.inventory(root, deadline)
        root_metadata = root.lstat()
        dump(out/'manifest.json', {'members': members, 'regular_bytes': total,
             'root_mode': stat.S_IMODE(root_metadata.st_mode), 'root_mtime_ns': root_metadata.st_mtime_ns})
        archive = out/'whole-owned-tree.tar.gz'
        preserve.write_archive(root, archive, members, deadline)
        record['archive_sha256'] = sha(archive)
        for _ in range(2):
            record['checks'].append(preserve.verify_archive(root, archive, members, deadline))
        current, current_total = preserve.inventory(root, deadline)
        if (current != members or current_total != total or sha(archive) != record['archive_sha256']
                or root.lstat().st_mode != root_metadata.st_mode
                or root.lstat().st_mtime_ns != root_metadata.st_mtime_ns
                or root.lstat().st_ino != root_metadata.st_ino
                or root.lstat().st_dev != root_metadata.st_dev):
            raise RuntimeError('owned tree/archive changed before cleanup')
        record['inactive_unit_before_cleanup'] = inactive_unit(unit)
        if any(sha(p) != record['source_sha256'][str(p)] for p in sources):
            raise RuntimeError('origin/source changed before cleanup')
        if footprint(category) > CAPS['coordinator']*1024**2 or footprint(BASE) > 1024**3:
            raise RuntimeError('retained native evidence exceeds cap')
        record.update(state='verified', regular_bytes=total, archive_bytes=archive.stat().st_size)
        dump(out/'receipt.json', record)
        preserve.check_deadline(deadline)
        shutil.rmtree(root)
        record.update(state='complete', original_removed=not os.path.lexists(root))
    except BaseException as error:
        record.update(state='failed', error=repr(error), original_removed=not os.path.lexists(root))
        raise
    finally:
        record.update(elapsed_s=time.monotonic()-started, category_bytes_after=footprint(category),
                      control_scratch_retained=scratch.exists())
        dump(out/'receipt.json', record)


if __name__ == '__main__':
    main()
