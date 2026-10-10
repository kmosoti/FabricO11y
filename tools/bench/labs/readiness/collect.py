#!/usr/bin/env python3
"""Archive launcher receipts and clean only this campaign's reviewed failures."""
import hashlib
import json
from pathlib import Path
import shutil
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits, STORAGE

DATA = ROOT / 'docs/experiments/benchmarks/data/readiness-labs-run-01'


def read(path):
    return json.loads(path.read_text())


def main():
    require_limits()
    output = DATA / 'cleanup.json'
    if output.exists():
        raise RuntimeError('cleanup receipt already exists')
    archived = DATA / 'launcher-receipts'
    archived.mkdir(exist_ok=False)
    receipts, launcher_intervals = [], []
    for path in (DATA / 'coordinator').glob('*/receipt.json'):
        record = read(path)
        if record['state'] == 'running':
            continue  # The enclosing final job writes its receipt after return.
        unit = Path(record['cgroup']).name.removesuffix('.service')
        launcher = ROOT / 'target/resource-containment/runs' / (unit + '.json')
        shutil.copyfile(launcher, archived / launcher.name)
        timing = read(launcher)
        launcher_intervals.append((timing['started_unix'], timing['started_unix'] + timing['elapsed_s']))
        receipts.append(record)
    ordered = sorted(launcher_intervals)
    for earlier, later in zip(ordered, ordered[1:]):
        if earlier[1] > later[0]:
            raise RuntimeError('project jobs overlapped')
    if sum(r['elapsed_s'] for r in receipts) > 3600:
        raise RuntimeError('campaign execution exceeded budget')
    for record in receipts:
        events = dict(line.split() for line in record['cgroup_final']['memory.events'].splitlines())
        if any(int(events[k]) for k in ('high', 'max', 'oom', 'oom_kill')):
            raise RuntimeError('unexpected memory pressure: ' + record['id'])
    review = read(DATA / 'memory/filter-review.json')
    for variant in ('reference', 'bounded'):
        result = review[variant]
        if not (result['logical_rows'] == 4096 and result['rejected_actual_trigrams'] == 0
                and result['missing_filter_control_rejected'] and result['cleared_filter_control_rejected']
                and result['file_hashes_authenticated'] and result['manifest_matches_original']):
            raise RuntimeError('failure inspection incomplete')
    counterexample = DATA / 'memory/manifest-layout-counterexample.json'
    counterexample_hash = hashlib.sha256(counterexample.read_bytes()).hexdigest()
    scan = read(DATA / 'query/scan/summary.json')
    audit = read(DATA / 'query/audit-reviewed.json')
    if scan['passed'] or scan['gates']['clock_offset_range_le_5ms']:
        raise RuntimeError('original invalid Scan result was changed')
    if not (audit['passed'] and audit['scratch_removed']
            and not audit['cells']['scan']['performance_eligible']
            and audit['cells']['off']['performance_eligible']):
        raise RuntimeError('accounting audit or failed-run classification differs')
    cleanup = []
    for job, expected_exit in (('m2-shapes', 1), ('q3-scan', 1), ('q4-accounting-review', 1), ('c1-reject-exit', 7)):
        record = read(DATA / 'coordinator' / job / 'receipt.json')
        if record['state'] != 'failed' or record['exit'] != expected_exit:
            raise RuntimeError('expected failure receipt differs: ' + job)
        unit = Path(record['cgroup']).name.removesuffix('.service')
        launcher = read(archived / (unit + '.json'))
        owned = STORAGE / 'evidence' / unit
        if launcher['retained_failure_evidence'] != str(owned) or owned.is_symlink():
            raise RuntimeError('failure directory ownership differs')
        if not owned.is_dir():
            raise RuntimeError('failure evidence unexpectedly absent')
        if job == 'c1-reject-exit':
            marker = owned / 'coordinator/c1-reject-exit/expected-failure.txt'
            if marker.read_text() != 'expected exit 7; preserve before cleanup\n':
                raise RuntimeError('negative-control retention failed')
            shutil.copyfile(marker, DATA / 'coordinator/c1-reject-exit/preserved-marker.txt')
        size = sum(p.stat().st_size for p in owned.rglob('*') if p.is_file())
        shutil.rmtree(owned)
        cleanup.append({'job': job, 'owned_path': str(owned), 'logical_bytes_removed': size,
                        'removed': not owned.exists(), 'original_failed_receipt_unchanged': True})
    report = {'at_unix_ns': time.time_ns(), 'failure_cleanup': cleanup,
              'preserved_counterexample_sha256': counterexample_hash,
              'preserved': 'original results, raw ledgers/manifests, protocols, source, seed, hashes, negative controls and forensic results',
              'completed_jobs_do_not_overlap': True,
              'prior_completed_execution_s': sum(r['elapsed_s'] for r in receipts),
              'enclosing_final_job': 'See coordinator/v1-checks/receipt.json after this command exits.'}
    output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__':
    main()
