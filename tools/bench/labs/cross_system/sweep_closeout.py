"""Bind final checks, preserve resource receipts, then remove authenticated binaries."""
import argparse
import copy
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess

from run_sweep_job import BASE, CAPS, ROOT, footprint, sha, dump
from resource_group import require_limits

STAMP_TOLERANCE_NS = 1_000_000_000  # permit one-second filesystem timestamp precision


def read(path):
    return json.loads(path.read_text())


def classify_receipt(receipt, runner_present, directory_name):
    """Marker distinguishes jobs from helper receipts; malformed jobs fail closed."""
    if not runner_present:
        if 'id' in receipt or 'cgroup_final' in receipt:
            raise RuntimeError('job-shaped receipt missing archived runner')
        return 'helper'
    required = {'id', 'lab', 'state', 'argv', 'started_unix_ns', 'scratch',
                'runner_sha256', 'limits'}
    if not required <= receipt.keys() or receipt['id'] != directory_name:
        raise RuntimeError('malformed coordinator job identity/schema')
    if (receipt['state'] not in ('running', 'passed', 'failed')
            or receipt['lab'] not in CAPS or not isinstance(receipt['argv'], list)
            or not receipt['argv'] or not isinstance(receipt['started_unix_ns'], int)):
        raise RuntimeError('malformed coordinator job fields')
    if receipt['state'] != 'running':
        required = {'exit', 'child_exit', 'elapsed_s', 'scratch_removed', 'cgroup_final'}
        if (not required <= receipt.keys() or not isinstance(receipt['scratch_removed'], bool)
                or not isinstance(receipt['elapsed_s'], (int, float)) or receipt['elapsed_s'] < 0):
            raise RuntimeError('malformed completed coordinator job')
        required = {'memory.peak', 'memory.events', 'memory.swap.current', 'cpu.stat', 'io.stat'}
        if not isinstance(receipt['cgroup_final'], dict) or not required <= receipt['cgroup_final'].keys():
            raise RuntimeError('incomplete completed cgroup observations')
    return 'job'


def check_boundary(receipt, check, head, mtime_ns, job):
    if (job['state'] != 'passed' or job['exit'] != 0 or job['child_exit'] != 0
            or not job['scratch_removed']):
        raise RuntimeError('final check coordinator did not complete successfully')
    if (receipt.get('check_id') != check['id'] or receipt.get('profile') != check['profile']
            or receipt.get('command') != check['command'] or receipt.get('candidate_commit') != head
            or receipt.get('result') != 'passed' or receipt.get('exit_code') != 0):
        raise RuntimeError('final verification identity/result differs')
    start = job['started_unix_ns']
    end = start + int(job['elapsed_s'] * 1_000_000_000)
    if not start - STAMP_TOLERANCE_NS <= mtime_ns <= end + STAMP_TOLERANCE_NS:
        raise RuntimeError('verification receipt outside final coordinator execution window')


def defect_controls():
    """Pure fixture controls invoke the same classifier and boundary used below."""
    job = dict(id='fixture', lab='coordinator', state='passed', argv=['fixture'],
               started_unix_ns=10_000_000_000, scratch='fixture', runner_sha256='fixture',
               limits={}, exit=0, child_exit=0, elapsed_s=2, scratch_removed=True,
               cgroup_final={k: '' for k in ('memory.peak', 'memory.events',
                                            'memory.swap.current', 'cpu.stat', 'io.stat')})
    check = dict(id='fixture-check', profile='fast', command=['fixture'])
    receipt = dict(check_id=check['id'], profile='fast', command=['fixture'],
                   candidate_commit='head', result='passed', exit_code=0)
    if classify_receipt({'state': 'complete'}, False, 'helper') != 'helper':
        raise RuntimeError('helper classifier positive rejected')
    classify_receipt(job, True, 'fixture')
    check_boundary(receipt, check, 'head', 11_000_000_000, job)
    rejected = []

    def reject(name, action):
        try:
            action()
        except RuntimeError:
            rejected.append(name)
        else:
            raise RuntimeError('closeout control accepted: ' + name)

    malformed = copy.deepcopy(job)
    del malformed['cgroup_final']
    reject('job_missing_cgroup', lambda: classify_receipt(malformed, True, 'fixture'))
    reject('job_missing_runner', lambda: classify_receipt(job, False, 'fixture'))
    reject('job_wrong_directory', lambda: classify_receipt(job, True, 'other'))
    for field, value in [('candidate_commit', 'stale'), ('check_id', 'wrong'),
                         ('command', ['wrong']), ('result', 'failed')]:
        changed = dict(receipt, **{field: value})
        reject('check_' + field, lambda: check_boundary(changed, check, 'head', 11_000_000_000, job))
    reject('check_before_window', lambda: check_boundary(receipt, check, 'head', 8_000_000_000, job))
    reject('check_after_window', lambda: check_boundary(receipt, check, 'head', 14_000_000_000, job))
    failed = dict(job, child_exit=1)
    reject('check_failed_child', lambda: check_boundary(receipt, check, 'head', 11_000_000_000, failed))
    return {'positive_helper_job_and_check': True, 'rejected': rejected,
            'scope': 'in-memory schema/identity/window defects; no workload or destructive fixture'}


def resolved_argument(argv, flag):
    if argv.count(flag) != 1 or argv.index(flag) + 1 >= len(argv):
        raise RuntimeError('required final coordinator argument missing/duplicated')
    value = Path(argv[argv.index(flag) + 1])
    return (value if value.is_absolute() else ROOT / value).resolve()


def final_bindings(jobs):
    by_id = {record['id']: (path, record) for path, record in jobs}
    paths = {'fast': BASE / 'coordinator/final-fast-01/checks',
             'documentation': BASE / 'coordinator/documentation-01/checks'}
    bindings = {}
    for profile, identifier in [('fast', 'final-fast-01'), ('documentation', 'final-docs-01')]:
        path, job = by_id[identifier]
        argv = job['argv']
        if profile == 'fast':
            if argv[:2] != ['env', 'CARGO_BUILD_JOBS=2']:
                raise RuntimeError('final fast build concurrency prefix differs')
            command = argv[2:]
            if (len(command) != 7 or command[:5] != ['cargo', 'xtask', 'checks', '--profile', 'fast']
                    or command[5] != '--receipts'):
                raise RuntimeError('final fast coordinator command differs')
            if resolved_argument(command, '--receipts') != paths[profile].resolve():
                raise RuntimeError('final fast receipt destination differs')
        else:
            if (len(argv) != 5 or argv[:2] != ['python3', '-B']
                    or (ROOT / argv[2]).resolve() != (ROOT / 'tools/bench/labs/cross_system/sweep_documentation.py').resolve()
                    or resolved_argument(argv, '--out') != paths[profile].parent.resolve()):
                raise RuntimeError('final documentation coordinator command differs')
            runtime_path = paths[profile].parent / 'runtime.json'
            runtime = read(runtime_path)
            expected = ['cargo', 'xtask', 'checks', '--profile', 'documentation',
                        '--receipts', str(paths[profile].resolve())]
            if runtime.get('command') != expected or runtime.get('exit') != 0 or not runtime.get('runtime_copy_removed'):
                raise RuntimeError('final documentation runtime/check binding differs')
        bindings[profile] = {'path': str(path), 'sha256': sha(path), 'argv': argv,
                             'job': job, 'checks_path': str(paths[profile])}
        if profile == 'documentation':
            bindings[profile]['runtime_sha256'] = sha(runtime_path)
    return bindings


def binary_preflight():
    confirmation = read(BASE / 'memory/confirmation-01/complete.json')
    if (confirmation.get('exit_code') != 0 or confirmation.get('pairs') != 12
            or confirmation.get('native_cells') != 24 or not confirmation.get('scratch_removed')):
        raise RuntimeError('fresh confirmation incomplete; retain frozen binaries')
    for size in (16, 64):
        for repeat in (0, 1):
            complete = read(BASE / 'memory' / f'target-{size}-repeat-{repeat}/complete.json')
            if (complete.get('exit_code') != 0 or complete.get('native_cells') != 18
                    or not complete.get('scratch_removed')):
                raise RuntimeError('storage grid incomplete; retain frozen binaries')
    manifest_path = BASE / 'memory/builds/build-manifest.json'
    manifest = read(manifest_path)
    owned = Path('/run/media/kmosoti/data/FabricO11y/cross-system-storage-sweep-01-binaries')
    expected = {Path(value['path']) for value in manifest['binaries'].values()}
    if owned.is_symlink() or owned.resolve(strict=True) != owned or set(owned.iterdir()) != expected:
        raise RuntimeError('unexpected owned binary directory entries')
    for info in manifest['binaries'].values():
        path = Path(info['path'])
        if path.parent != owned or path.is_symlink() or not path.is_file() or sha(path) != info['sha256']:
            raise RuntimeError('frozen binary identity differs; retain all binaries')
    return owned, expected, {'root': str(owned), 'removed_bytes': sum(p.stat().st_size for p in expected),
                            'authenticated_against': str(manifest_path), 'manifest_sha256': sha(manifest_path),
                            'binaries': manifest['binaries'], 'removed': False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path)
    parser.add_argument('--remove-storage-binaries', action='store_true')
    parser.add_argument('--controls-only', action='store_true')
    args = parser.parse_args()
    require_limits()
    if args.controls_only:
        if args.remove_storage_binaries or args.out is not None:
            parser.error('controls-only does not write receipts or remove binaries')
        print(json.dumps(defect_controls(), indent=2))
        return
    if args.out is None:
        parser.error('--out required for closeout')
    check_dir = args.out.parent / 'verification'
    if args.out.exists() or check_dir.exists():
        raise RuntimeError('fresh closeout receipt and verification directory required')
    report = {'helper_sha256': sha(Path(__file__)), 'checks': [], 'cleanup': [],
              'defect_controls': defect_controls(), 'helper_receipts': []}
    jobs = []
    for path in sorted((BASE / 'coordinator').glob('*/receipt.json')):
        receipt = read(path)
        runner = path.parent / 'runner.py.gz'
        kind = classify_receipt(receipt, runner.exists(), path.parent.name)
        if kind == 'helper':
            report['helper_receipts'].append({'path': str(path), 'sha256': sha(path),
                                             'state': receipt.get('state'),
                                             'elapsed_s': receipt.get('elapsed_s', 0)})
        else:
            if hashlib.sha256(gzip.decompress(runner.read_bytes())).hexdigest() != receipt['runner_sha256']:
                raise RuntimeError('coordinator archived runner identity differs')
            jobs.append((path, receipt))
    completed = [(path, r) for path, r in jobs if r['state'] != 'running']
    report['final_check_bindings'] = bindings = final_bindings(jobs)
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    report['candidate_commit'] = head
    report['mtime_tolerance_ns'] = STAMP_TOLERANCE_NS
    pending_copies = []
    config = read(ROOT / 'xtask/checks.json')
    for check in config['checks']:
        if check['profile'] not in bindings:
            continue
        binding = bindings[check['profile']]
        path = Path(binding['checks_path']) / (check['id'] + '.json')
        receipt = read(path)
        check_boundary(receipt, check, head, path.stat().st_mtime_ns, binding['job'])
        digest = sha(path)
        pending_copies.append((path, check_dir / path.name, digest))
        report['checks'].append({'id': check['id'], 'profile': check['profile'],
                                'source_path': str(path), 'source_mtime_ns': path.stat().st_mtime_ns,
                                'receipt_sha256': digest, 'receipt': receipt})
    report['preserved_failures'] = []
    for relative in ('query/sweep-01/preserved-scratch-01/receipt.json',
                     'query/vortex-01/preserved-scratch-01/receipt.json',
                     'query/rowset-locality-report-01/preserved-scratch-01/receipt.json',
                     'source/empty-failure-preservation/receipt.json'):
        path = BASE / relative
        receipt = read(path)
        archive = path.parent / 'whole-owned-tree.tar.gz'
        original = Path(receipt['root'])
        if (receipt.get('state') != 'complete' or not receipt.get('original_removed')
                or len(receipt.get('checks', [])) != 2
                or not all(c.get('exact_manifest_readback') for c in receipt['checks'])
                or sha(archive) != receipt['archive_sha256']):
            raise RuntimeError('failure preservation receipt/archive incomplete')
        report['preserved_failures'].append({'path': str(path), 'receipt_sha256': sha(path),
            'archive_path': str(archive), 'archive_sha256': sha(archive), 'regular_bytes': receipt['regular_bytes'],
            'original_root': str(original), 'original_root_currently_exists': os.path.lexists(original),
            'recorded_original_removed': receipt['original_removed'], 'checks': receipt['checks'],
            'origin_sha256': receipt.get('origin_sha256', {})})
        if os.path.lexists(original):
            raise RuntimeError('preserved original root remains; retain frozen binaries')
    path = BASE / 'query/rowset-01/failure.json'
    receipt = read(path)
    archive = path.parent / 'failed-state.tar.gz'
    original = Path(receipt['retained_scratch'])
    if (not receipt.get('scratch_removed') or not receipt.get('payload_readback')
            or sha(archive) != receipt['complete_archive_sha256']
            or archive.stat().st_size != receipt['complete_archive_bytes']
            or os.path.lexists(original)):
        raise RuntimeError('original RowSet failure preservation incomplete')
    report['preserved_failures'].append({'path': str(path), 'receipt_sha256': sha(path),
        'archive_path': str(archive), 'archive_sha256': sha(archive),
        'archive_bytes': archive.stat().st_size, 'recorded_payload_readback': True,
        'original_root': str(original), 'original_root_currently_exists': False,
        'recorded_original_removed': True,
        'scope': 'original harness full archive; distinct from later two-readback preservation helper'})
    usage = {lab: footprint(BASE / lab) for lab in CAPS}
    if any(usage[lab] > cap * 1024**2 for lab, cap in CAPS.items()) or footprint(BASE) > 2 * 1024**3:
        raise RuntimeError('evidence allocation exceeded')

    def counters(value):
        return dict(line.split() for line in value.splitlines())

    report['resources'] = {
        'completed_job_seconds': sum(r['elapsed_s'] for _, r in completed),
        'coordinator_ledger_charged_seconds': sum(r['elapsed_s'] for _, r in completed)
            + sum(r['elapsed_s'] for r in report['helper_receipts'] if r['state'] != 'running'),
        'round_allocation_seconds': 7200,
        'jobs': [dict({k: r.get(k) for k in ('id', 'lab', 'state', 'exit', 'child_exit', 'error',
                        'elapsed_s', 'scratch', 'scratch_removed', 'evidence_census_error', 'limits', 'cgroup_final')},
                      receipt_path=str(path), receipt_sha256=sha(path),
                      original_scratch_currently_exists=os.path.lexists(r['scratch'])) for path, r in completed],
        'current_running_job_ids': [r['id'] for _, r in jobs if r['state'] == 'running'],
        'maximum_completed_cgroup_peak_bytes': max(int(r['cgroup_final']['memory.peak']) for _, r in completed),
        'all_recorded_swap_zero': all(int(r['cgroup_final']['memory.swap.current']) == 0 for _, r in completed),
        'all_recorded_oom_zero': all(int(counters(r['cgroup_final']['memory.events']).get('oom', 0)) == 0
                                   and int(counters(r['cgroup_final']['memory.events']).get('oom_kill', 0)) == 0
                                   for _, r in completed),
        'evidence_bytes_by_category': usage, 'category_caps_mib': CAPS,
        'aggregate_evidence_bytes': footprint(BASE),
        'retained_failed_job_ids': [r['id'] for _, r in completed if r['state'] == 'failed'],
        'data_drive_free_bytes': shutil.disk_usage(os.environ['FABRIC_SCRATCH_ROOT']).free,
        'cpu_scope': 'Default cgroup cpu.max is unlimited. Native workloads serialized; builds use two jobs; query/Vortex affinity is two CPUs.',
        'interpretation': 'Current closeout excluded from completed totals. Historical scratch_removed flags stay unchanged; current existence and later preservation are reported separately.'}
    report['checks_all_passed'] = True  # every configured final check was validated above
    cleanup_plan = binary_preflight() if args.remove_storage_binaries else None
    # Complete report preflight and verified receipt preservation BEFORE any binary deletion.
    json.dumps(report)
    check_dir.mkdir(parents=True, exist_ok=False)
    for source, target, digest in pending_copies:
        shutil.copyfile(source, target)
        if sha(source) != digest or sha(target) != digest:
            raise RuntimeError('verification receipt preservation differs; retain binaries')
    if cleanup_plan:
        report['cleanup'].append(cleanup_plan[2])
    dump(args.out, report)
    # Include the newly preserved checks/report in the last allocation check.
    usage = {lab: footprint(BASE / lab) for lab in CAPS}
    if any(usage[lab] > cap * 1024**2 for lab, cap in CAPS.items()) or footprint(BASE) > 2 * 1024**3:
        raise RuntimeError('closeout preservation exceeds evidence allocation; retain binaries')
    report['resources']['evidence_bytes_by_category'] = usage
    report['resources']['aggregate_evidence_bytes'] = footprint(BASE)
    dump(args.out, report)
    if cleanup_plan:
        owned, expected, entry = cleanup_plan
        # Recheck the entire identity immediately before mutation.
        binary_preflight()
        for path in expected:
            path.unlink()
        owned.rmdir()
        entry['removed'] = not owned.exists()
        dump(args.out, report)
    print(json.dumps({'checks': len(report['checks']), 'checks_all_passed': report['checks_all_passed'],
                      'resources': report['resources'], 'cleanup': report['cleanup']}, indent=2))


if __name__ == '__main__':
    main()
