#!/usr/bin/env python3
"""Summarize retained screen evidence; never change a cell's acceptance result."""
import gzip
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits

DATA = ROOT / 'docs/experiments/benchmarks/data/dev-small-labs-run-02'
PHASES = ('normal', 'burst', 'recovery')


def load(path):
    return json.loads(path.read_text())


def phase(timestamp, epoch):
    second = (timestamp - epoch) / 1e9
    return PHASES[int(second // 60)] if 0 <= second < 180 else None


def cell(path):
    summary = load(path / 'summary.json')
    audit = load(path / 'independent-audit.json')
    obs = summary['observation']
    epoch = obs['epoch_ns']
    with gzip.open(path / 'resources.json.gz', 'rt') as stream:
        resources = json.load(stream)
    phases = {}
    for name in PHASES:
        rows = [row for row in resources if phase(row['wall_ns'], epoch) == name]
        cpu = obs['cpu'][name]['processes']['server']
        phases[name] = {
            'samples': len(rows),
            'sampled_peak_server_rss_mib': max(row['server']['rss_kib'] for row in rows) / 1024,
            'server_cpu_cores': summary['measurement']['phases'][name]['server_cpu_cores'],
            'server_user_seconds': cpu['user_seconds'],
            'server_system_seconds': cpu['system_seconds'],
            'server_io': cpu['io_delta'],
            'rates': obs['rates'][name],
        }
    transitions = []
    for previous, current in zip(resources, resources[1:]):
        if any(previous[k] != current[k] for k in ('segments', 'sealed_files')):
            transitions.append({
                'bracket_from_ns': previous['wall_ns'], 'bracket_to_ns': current['wall_ns'],
                'relative_to_offer_start_s': (current['wall_ns'] - epoch) / 1e9,
                'segments_before': previous['segments'], 'segments_after': current['segments'],
                'sealed_before': previous['sealed_files'], 'sealed_after': current['sealed_files'],
            })
    queries = []
    if (path / 'queries.jsonl.gz').exists():
        with gzip.open(path / 'queries.jsonl.gz', 'rt') as stream:
            queries = [json.loads(line) for line in stream]
    for transition in transitions:
        transition['requests_intersecting_bracket'] = sum(
            row['start_ns'] < transition['bracket_to_ns'] and
            row['end_ns'] > transition['bracket_from_ns'] for row in queries)
    receipt = load(DATA / 'coordinator' / path.name / 'receipt.json')
    return {
        'cell': path.name, 'command_exit': receipt['exit'],
        'passed': summary['passed'], 'independent_audit_passed': audit['passed'],
        'audit_problems': audit['problems'], 'timing_valid': summary['measurement']['valid_for_timing'],
        'live_logs': audit['counts']['source_logs'], 'history_logs': summary['history']['rows'],
        'recovered_logs': summary['recovered_logs'], 'history_prefix_sha256': summary['history']['prefix_sha256'],
        'peak_server_hwm_mib': summary['server_peak_rss_mib'],
        'peak_node_hwm_mib': summary['node_peak_rss_mib'],
        'balanced_server_cpu_cores': sum(row['server_cpu_cores'] for row in phases.values()) / 3,
        'phase_peak_server_rss_mib': max(row['sampled_peak_server_rss_mib'] for row in phases.values()),
        'server_timed_cpu_seconds': summary['server_cpu_seconds'],
        'ack_observed_p99_ms': summary['latency_ms']['all']['ack']['p99'],
        'ack_boundary': 'Collection observation to ACK stdout observation; neither source-write latency nor an instrumented server durability timestamp.',
        'retries': summary['non_ack_attempts'], 'phases': phases,
        'queries': obs['queries'], 'visibility': obs['visibility'],
        'clock': summary['measurement']['clock'],
        'timed_cgroup_peak_bytes': obs['cgroup_peak_bytes'],
        'timed_cgroup_last': obs['cgroup_last'],
        'whole_job_cgroup_final': receipt['cgroup_final'],
        'whole_job_elapsed_seconds': receipt['elapsed_s'],
        'whole_job_peak_sampled_scratch_bytes': receipt['peak_sampled_scratch_bytes'],
        'peak_trial_disk_bytes': summary['peak_trial_disk_bytes'],
        'peak_pending_batches': obs['peak_pending_batches'],
        'max_sealed_files_waiting': summary['max_sealed_files_waiting'],
        'segments_final': summary['segments_final'], 'transitions': transitions,
        'transition_limit': 'One-second sample brackets do not identify exact build/publication intervals; intersecting requests do not prove overlap with a running builder.',
        'cleanup': load(path / 'cleanup.json'),
    }


def main():
    require_limits()
    output = DATA / 'consolidated.json'
    if output.exists():
        raise RuntimeError('refusing to overwrite consolidated evidence')
    cells = {}
    for lab in ('memory', 'query'):
        for path in sorted((DATA / lab).iterdir()):
            if (path / 'summary.json').exists() and (path / 'independent-audit.json').exists():
                cells[path.name] = cell(path)
    comparisons = {}
    for name, baseline, candidate in (
        ('query_effect_fresh', 'c1-1', 'c1-3'),
        ('query_effect_history', 'c1-4', 'c1-2'),
        ('history_effect_queries_off', 'c1-1', 'c1-4'),
        ('history_effect_queries_on', 'c1-3', 'c1-2'),
        ('lower_rate_with_history_queries', 'c1-2', 'c1-5'),
    ):
        if baseline in cells and candidate in cells:
            a, b = cells[baseline], cells[candidate]
            valid = all(x['passed'] and x['timing_valid'] and x['independent_audit_passed'] for x in (a, b))
            comparisons[name] = {'baseline': baseline, 'candidate': candidate, 'eligible': valid}
            if valid:
                for metric in ('balanced_server_cpu_cores', 'phase_peak_server_rss_mib'):
                    ratio = b[metric] / a[metric]
                    comparisons[name][metric] = {'ratio': ratio, 'change_percent': (ratio - 1) * 100,
                        'meets_15_percent_nomination': abs(ratio - 1) >= 0.15}
    if all(name in cells and cells[name]['passed'] and cells[name]['timing_valid']
           and cells[name]['independent_audit_passed'] for name in ('c1-1','c1-2','c1-3','c1-4')):
        comparisons['history_query_additive_interaction'] = {
            'formula': '(c1-2 - c1-4) - (c1-3 - c1-1)',
            **{metric: (cells['c1-2'][metric] - cells['c1-4'][metric]) -
                       (cells['c1-3'][metric] - cells['c1-1'][metric])
               for metric in ('balanced_server_cpu_cores','phase_peak_server_rss_mib')},
        }
    result = {'cells': cells, 'comparisons': comparisons,
        'nonempty_history_prefixes': sorted({row['history_prefix_sha256']
            for name, row in cells.items() if name.startswith('c1') and row['history_logs']}),
        'interpretation': 'Single-cell diagnostic nominations only; no significance, universal capacity or release qualification.'}
    output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'cells': list(cells), 'output': str(output)}))


if __name__ == '__main__':
    main()
