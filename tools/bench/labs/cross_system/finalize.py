"""Consolidate receipts and update the non-executable research coordination queue."""
import json
from pathlib import Path
import shutil

from source_fetch import require_limits
from run_job import BASE, CAPS, footprint


def read(path):
    return json.loads(path.read_text())


def dump(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def main():
    require_limits()
    evidence = BASE / 'verification'
    evidence.mkdir(exist_ok=True)
    config = read(Path('xtask/checks.json'))
    selected = [r for r in config['checks'] if r['profile'] in ('fast', 'documentation')]
    checks = []
    for check in selected:
        path = Path('target/verification/receipts') / (check['id'] + '.json')
        if not path.exists():
            continue
        receipt = read(path)
        shutil.copyfile(path, evidence / path.name)
        checks.append({'id': check['id'], 'profile': check['profile'], 'receipt': receipt})
    jobs = [read(p) for p in (BASE / 'coordinator').glob('*/receipt.json')]
    completed = [r for r in jobs if r['state'] != 'running']
    usage = {lab: footprint(BASE / lab) for lab in CAPS}
    peaks = [int(r.get('cgroup_final', {}).get('memory.peak', 0)) for r in completed]
    summary = {'completed_job_wall_seconds': sum(r.get('elapsed_s', 0) for r in completed),
               'registered_continuation_seconds': 7200, 'maximum_job_cgroup_peak_bytes': max(peaks),
               'evidence_bytes_by_category': usage, 'category_caps_mib': CAPS,
               'aggregate_evidence_bytes': footprint(BASE), 'aggregate_cap_bytes': 1024**3,
               'all_recorded_cgroups_zero_swap': all(int(r['cgroup_final']['memory.swap.current']) == 0
                   for r in completed if 'cgroup_final' in r),
               'retained_failed_jobs': [r['id'] for r in completed if r['state'] == 'failed'],
               'checks': checks, 'running_job_ids_excluded_from_elapsed': [r['id'] for r in jobs if r['state'] == 'running']}
    if summary['aggregate_evidence_bytes'] > 1024**3 or any(usage[k] > CAPS[k]*1024**2 for k in CAPS):
        raise RuntimeError('final evidence allocation exceeded')
    dump(BASE / 'resource-summary.json', summary)
    queue_path = Path('docs/research/cross-system-lab-queue.json')
    queue = read(queue_path)
    queue['status'] = 'source dissection complete; finite census and diagnostic follow-ups executed; broader research remains'
    queue['execution_admission'] = 'completed continuation under verified containment; future cells require their own concrete registration and admission'
    queue['native_workloads_dispatched_historical'] = queue.pop('native_workloads_dispatched', 5)
    queue['continuation'] = {
        'protocol': '../experiments/benchmarks/cross-system-continuation-protocol.md',
        'findings': '../experiments/benchmarks/cross-system-continuation-findings.md',
        'source_synthesis': 'cross-system-source-synthesis.md',
        'source_archives_inventoried': 21, 'additional_selected_repository_retrievals': 3,
        'native_probe_children_executed': 78,
        'memory': 'census01 failed layout comparison; census02 passed with RSS qualification; census03 isolated decoder and reused exact archives',
        'query': '512 census chains plus1024 small-profile ablation chains; all independent verdicts accepted',
        'operations': '18 diagnostic2ms cells and18 registered10ms cells; no universal timing gain',
        'production_default_changes': False, 'goal_complete': False}
    pins = queue['source_revisions']
    history = queue.setdefault('source_revision_history', {})
    supplements = read(BASE / 'source/supplements-01/receipt.json')
    for spec in read(Path('tools/bench/labs/cross_system/source_catalog.json'))['repositories']:
        identity, key = spec['id'], spec['pin_key']
        receipt = read(BASE / 'source' / identity / 'receipt.json')
        revision = supplements.get(identity + '-current', receipt)['revision']
        if key in pins and pins[key] != revision:
            history.setdefault(key, []).append({'revision': pins[key],
                'reason': 'historical retrieval returned404; replacement immutable pin preserved in supplement01'})
        pins[key] = revision
    queue['queue_interpretation'] = 'Items coordinate research, not executable dispatch; source_revisions supplies downloader pins.'
    replacements = {
        'M1': ('offline slice complete; simultaneous service reservations still pending',
               'six repaired cells:65–66% less requested-live build peak and46–54% lower whole-worker RSS on16MiB fixtures'),
        'Q1': ('log slice complete; mixed signals and query-only full-chain timing pending',
               'eight variants,512 accepted chains/3520pages;72 rejected controls; block projection dominates wide selected queries'),
        'Q2': ('small-profile existing-code ablation complete without nomination; broader ownership work pending',
               '16variants,1024 accepted chains/7040pages;144 rejected controls;~2.01MiB less requested allocation, no compelling CPU benefit'),
    }
    for item in queue['items']:
        if item['id'] in replacements:
            item['state'], item['run_outcome'] = replacements[item['id']]
            item['registration'] = queue['continuation']['protocol']
            item['accountable_pi'] = 'capacity PI' if item['id'] == 'M1' else 'query PI'
        elif item['id'] == 'O1':
            item['continuation_outcome'] = 'registered18-cell Store/intake prefix comparison;144acceptedchains/36rejectedcontrols; one resolved skewed byte-time pair, other bounds overlap'
            item['continuation_registration'] = queue['continuation']['protocol']
        elif item.get('run_outcome') == 'not run; environment unavailable':
            item['historical_preparation_outcome'] = item['run_outcome']
            item['run_outcome'] = 'not executed in this continuation; requires a scoped fixture and resource admission'
    queue['generative_exploration']['state'] = 'source mechanisms compiled; existing D1 and selective-ownership hypotheses discriminated; no seeded candidate sampler or new production nomination'
    dump(queue_path, queue)
    print(json.dumps({k: v for k, v in summary.items() if k != 'checks'}, indent=2))


if __name__ == '__main__':
    main()
