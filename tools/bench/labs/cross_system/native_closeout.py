"""Authenticate fresh native checks and finite owned binary copies before cleanup."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import subprocess

from run_native_job import BASE, CAPS, PROTOCOL, ROOT, STORAGE, dump, footprint, load_ledgers, ledger_totals, require_limits, sha
from sweep_closeout import check_boundary, defect_controls, resolved_argument

MEMORY_BINARIES = STORAGE / 'native-ownership-binaries'
QUERY_NAMES = {'plain-legacy', 'plain-key-first', 'counted-legacy', 'counted-key-first'}


def read(path):
    return json.loads(path.read_text())


def path_and_hash(path, parent, actual, expected):
    if path.parent != parent or path.name in ('', '.', '..') or actual != expected:
        raise RuntimeError('binary path/hash identity differs')


def regular(path):
    if path.is_symlink() or not path.is_file() or path.resolve(strict=True) != path:
        raise RuntimeError('nonregular/linked binary or receipt path')


def check_job(identifier):
    if not identifier.replace('-', '').replace('_', '').isalnum():
        raise RuntimeError('noncanonical final job id')
    path = BASE/'coordinator'/identifier/'receipt.json'
    regular(path)
    record = read(path)
    if (record.get('id') != identifier or record.get('state') != 'passed'
            or record.get('exit') != 0 or record.get('child_exit') != 0
            or not record.get('scratch_removed')):
        raise RuntimeError('final check job incomplete/failed')
    runner = path.parent/'runner.py.gz'
    regular(runner)
    if hashlib.sha256(gzip.decompress(runner.read_bytes())).hexdigest() != record['runner_sha256']:
        raise RuntimeError('final job archived runner differs')
    return path, record


def check_fast_command(argv, destination):
    if argv[:3] == ['env', 'CARGO_BUILD_JOBS=2', 'RUST_TEST_THREADS=2']:
        argv = argv[3:]
    elif argv[:2] == ['env', 'CARGO_BUILD_JOBS=2']:
        argv = argv[2:]
    if (len(argv) != 7 or argv[:6] != ['cargo', 'xtask', 'checks', '--profile', 'fast', '--receipts']
            or resolved_argument(argv, '--receipts') != destination.resolve()):
        raise RuntimeError('fresh fast command/destination differs')


def binary_plan(manifest_path, query_roots):
    regular(manifest_path)
    manifest = read(manifest_path)
    if set(manifest['binaries']) != {'baseline', 'candidate'}:
        raise RuntimeError('unknown memory binary arms')
    owned = MEMORY_BINARIES
    if owned.is_symlink() or owned.resolve(strict=True) != owned:
        raise RuntimeError('owned memory directory identity differs')
    paths = {Path(entry['path']) for entry in manifest['binaries'].values()}
    if set(owned.iterdir()) != paths:
        raise RuntimeError('unknown owned memory directory entries')
    plan = []
    for name, entry in manifest['binaries'].items():
        path = Path(entry['path'])
        regular(path)
        if path.name != name or entry['build'].get('exit_code') != 0:
            raise RuntimeError('memory build/name identity differs')
        path_and_hash(path, owned, sha(path), entry['sha256'])
        plan.append({'path': str(path), 'parent': str(owned), 'bytes': path.stat().st_size,
                     'sha256': entry['sha256'], 'decoded': False,
                     'provenance_path': str(manifest_path), 'provenance_sha256': sha(manifest_path)})
    for root in query_roots:
        if (root.parent != BASE/'query' or root.is_symlink() or root.resolve(strict=True) != root):
            raise RuntimeError('query root outside exact native query boundary')
        provenance_path = root/'provenance.json'
        regular(provenance_path)
        provenance = read(provenance_path)
        hashes = provenance['binary_sha256']
        if set(hashes) != QUERY_NAMES:
            raise RuntimeError('unknown query binary arms')
        parent = root/'binaries'
        if parent.is_symlink() or set(parent.iterdir()) != {parent/(name+'.gz') for name in QUERY_NAMES}:
            raise RuntimeError('unknown query binary directory entries')
        for name, expected in hashes.items():
            path = parent/(name+'.gz')
            regular(path)
            hashed = hashlib.sha256()
            decoded_bytes = 0
            with gzip.open(path, 'rb') as payload:
                while block := payload.read(65536):
                    decoded_bytes += len(block)
                    if decoded_bytes > 64*1024**2:
                        raise RuntimeError('query binary decoded ceiling exceeded')
                    hashed.update(block)
            path_and_hash(path, parent, hashed.hexdigest(), expected)
            plan.append({'path': str(path), 'parent': str(parent), 'bytes': path.stat().st_size,
                'sha256': sha(path), 'decoded': True, 'decoded_sha256': expected,
                'decoded_bytes': decoded_bytes, 'provenance_path': str(provenance_path),
                'provenance_sha256': sha(provenance_path)})
    return plan


def controls():
    result = defect_controls()
    parent = MEMORY_BINARIES
    path_and_hash(parent/'baseline', parent, 'same', 'same')
    for name, path, actual in [('bad_binary_hash', parent/'baseline', 'altered'),
                               ('outside_binary_path', STORAGE/'cargo/baseline', 'same')]:
        try:
            path_and_hash(path, parent, actual, 'same')
        except RuntimeError:
            result['rejected'].append(name)
        else:
            raise RuntimeError('native closeout control accepted: ' + name)
    destination = BASE/'coordinator/command-control/checks'
    cargo = ['cargo', 'xtask', 'checks', '--profile', 'fast', '--receipts', str(destination)]
    check_fast_command(['env', 'CARGO_BUILD_JOBS=2', 'RUST_TEST_THREADS=2', *cargo], destination)
    for name, command in [('unexpected_test_concurrency', ['env', 'CARGO_BUILD_JOBS=2', 'RUST_TEST_THREADS=3', *cargo]),
                          ('omitted_full_profile', ['env', 'CARGO_BUILD_JOBS=2', 'RUST_TEST_THREADS=2', *cargo, '--only', 'clippy'])]:
        try:
            check_fast_command(command, destination)
        except RuntimeError:
            result['rejected'].append(name)
        else:
            raise RuntimeError('native closeout control accepted: ' + name)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--fast-job', required=True)
    parser.add_argument('--docs-job', required=True)
    parser.add_argument('--docs-path', type=Path, required=True)
    parser.add_argument('--memory-build-manifest', type=Path, required=True)
    parser.add_argument('--query-screen', type=Path)
    parser.add_argument('--query-confirm', type=Path)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    require_limits()
    args.out = args.out.resolve()
    if not args.out.is_relative_to(BASE.resolve()) or args.out.exists():
        raise RuntimeError('fresh native evidence output required')
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    fast_path, fast = check_job(args.fast_job)
    docs_path, docs = check_job(args.docs_job)
    fast_checks = fast_path.parent/'checks'
    check_fast_command(fast['argv'], fast_checks)
    docs_output = args.docs_path.resolve(strict=True)
    docs_output.relative_to(BASE.resolve())
    if (len(docs['argv']) != 5 or docs['argv'][:2] != ['python3', '-B']
            or (ROOT/docs['argv'][2]).resolve() != (ROOT/'tools/bench/labs/cross_system/sweep_documentation.py').resolve()
            or resolved_argument(docs['argv'], '--out') != docs_output):
        raise RuntimeError('documentation helper output differs')
    runtime_path = docs_output/'runtime.json'
    regular(runtime_path)
    runtime = read(runtime_path)
    docs_checks = docs_output/'checks'
    if (runtime.get('command') != ['cargo', 'xtask', 'checks', '--profile', 'documentation',
                                   '--receipts', str(docs_checks)]
            or runtime.get('exit') != 0 or not runtime.get('runtime_copy_removed')):
        raise RuntimeError('documentation runtime/check command differs')
    checks = []
    for spec in read(ROOT/'xtask/checks.json')['checks']:
        if spec['profile'] not in ('fast', 'documentation'):
            continue
        parent, job = (fast_checks, fast) if spec['profile'] == 'fast' else (docs_checks, docs)
        path = parent/(spec['id']+'.json')
        regular(path)
        receipt = read(path)
        check_boundary(receipt, spec, head, path.stat().st_mtime_ns, job)
        checks.append({'path': str(path), 'sha256': sha(path), 'receipt': receipt,
                       'mtime_ns': path.stat().st_mtime_ns})
    if (sum(c['receipt']['profile'] == 'fast' for c in checks) != 17
            or sum(c['receipt']['profile'] == 'documentation' for c in checks) != 3):
        raise RuntimeError('final configured check counts differ from 17+3')
    current = Path(os.environ['FABRIC_SCRATCH_ROOT']).name
    jobs = []
    for path in sorted((BASE/'coordinator').glob('*/receipt.json')):
        record = read(path)
        if record.get('state') == 'running' and record.get('id') != current:
            raise RuntimeError('other native job still active')
        if record.get('state') != 'running':
            jobs.append({'receipt_path': str(path), 'receipt_sha256': sha(path), 'receipt': record})
    query_roots = [p.resolve(strict=True) for p in (args.query_screen, args.query_confirm) if p is not None]
    if len(set(query_roots)) != len(query_roots):
        raise RuntimeError('duplicate query cleanup roots')
    manifest = args.memory_build_manifest.resolve(strict=True)
    manifest.relative_to(BASE/'memory')
    plan = binary_plan(manifest, query_roots)
    ledgers, _ = load_ledgers()
    # Current closeout is running and intentionally excluded from completed accounting.
    ledgers['native-frontier-01'] = [r for r in ledgers['native-frontier-01'] if r['state'] != 'running']
    report = {'state': 'authenticated', 'helper_sha256': sha(Path(__file__)),
        'boundary_checker_sha256': sha(Path(__file__).with_name('sweep_closeout.py')),
        'protocol_sha256': sha(PROTOCOL), 'candidate_commit': head, 'checks': checks,
        'final_job_bindings': {'fast': {'path': str(fast_path), 'sha256': sha(fast_path), 'argv': fast['argv']},
                               'docs': {'path': str(docs_path), 'sha256': sha(docs_path), 'argv': docs['argv'],
                                        'runtime_sha256': sha(runtime_path)}},
        'jobs': jobs, 'ledger_totals': ledger_totals(ledgers), 'rejected_controls': controls(),
        'cleanup': plan, 'shared_cargo_cache_retained': os.environ.get('CARGO_TARGET_DIR'),
        'interpretation': 'Full job receipts retain cgroup peaks/events/swap/CPU/IO and failures; current closeout boundary is its final coordinator receipt.'}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    dump(args.out, report)  # preflight evidence before removing replaceable binaries
    usage = {lab: footprint(BASE/lab) for lab in CAPS}
    if any(usage[k] > CAPS[k]*1024**2 for k in CAPS) or footprint(BASE) > 1024**3:
        raise RuntimeError('closeout evidence exceeds allocation; retain all binaries')
    if binary_plan(manifest, query_roots) != plan:
        raise RuntimeError('binary plan changed before removal')
    for entry in plan:
        path = Path(entry['path'])
        path.unlink()
        entry['removed'] = not path.exists()
        dump(args.out, report)  # preserve partial cleanup progress if a later removal fails
    report['removed_binary_directories'] = []
    for parent in sorted({Path(e['parent']) for e in plan}):
        parent.rmdir()
        report['removed_binary_directories'].append(str(parent))
        dump(args.out, report)
    report.update(state='complete', cleanup_removed_bytes=sum(e['bytes'] for e in plan),
                  evidence_bytes_by_category={lab: footprint(BASE/lab) for lab in CAPS})
    dump(args.out, report)


if __name__ == '__main__':
    main()
