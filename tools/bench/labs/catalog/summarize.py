"""Derive paired summaries from retained catalog experiment receipts."""
import json
from pathlib import Path
import statistics
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits


def read(path):
    return json.loads(path.read_text())


def main():
    require_limits()
    base = ROOT / 'docs/experiments/benchmarks/data'
    result = {'interpretation': 'isolated mechanism summaries, not service performance'}
    spill = base / 'catalog-spill-run-01'
    if (spill / 'result.json').exists():
        trials = read(spill / 'result.json')['trials']
        rows = []
        for candidate in ('encode', 'decode', 'combined'):
            pairs = []
            for repeat in (1, 2, 3):
                selected = {(t['build'], t['variant']): t for t in trials
                            if t['candidate'] == candidate and t['repetition'] == repeat}
                baseline = selected['plain', 'baseline']
                treatment = selected['plain', candidate]
                def cpu(variant):
                    receipt = read(spill / f'plain-{candidate}-{repeat}-{variant}.command.json')
                    return receipt['whole_child_user_s'] + receipt['whole_child_system_s']
                b = selected['counted', 'baseline']['allocator']
                c = selected['counted', candidate]['allocator']
                pairs.append({'pair': repeat,
                    'plain_phase_wall_ratio': treatment['phase_wall_s'] / baseline['phase_wall_s'],
                    'plain_child_cpu_ratio': cpu(candidate) / cpu('baseline'),
                    'allocation_call_ratio': c['successful_alloc_realloc_calls'] / b['successful_alloc_realloc_calls'],
                    'allocation_byte_ratio': c['cumulative_requested_bytes'] / b['cumulative_requested_bytes'],
                    'baseline_incremental_peak': b['incremental_peak'],
                    'candidate_incremental_peak': c['incremental_peak']})
            rows.append({'candidate': candidate, 'pairs': pairs,
                'median_plain_phase_wall_ratio': statistics.median(p['plain_phase_wall_ratio'] for p in pairs),
                'median_plain_child_cpu_ratio': statistics.median(p['plain_child_cpu_ratio'] for p in pairs)})
        result['spill'] = rows
    metadata = base / 'catalog-metadata-run-01'
    if (metadata / 'trials.json').exists():
        trials = read(metadata / 'trials.json')
        result['metadata'] = []
        for readers in (1, 4):
            pairs = []
            for pair in (1, 2, 3):
                t = {(r['variant'], r['mode']): read(metadata / r['output'])
                     for r in trials if r['readers'] == readers and r['pair'] == pair}
                if len(t) != 4:
                    continue
                b, c = t['counted', 'clone'], t['counted', 'arc']
                pairs.append({'pair': pair,
                    'plain_wall_ratio': t['plain', 'arc']['wall_ns'] / t['plain', 'clone']['wall_ns'],
                    'allocation_byte_ratio': c['allocation_after'][2] / b['allocation_after'][2],
                    'allocation_call_ratio': c['allocation_after'][3] / b['allocation_after'][3],
                    'baseline_incremental_peak': b['allocation_after'][1] - b['allocation_before'][0],
                    'candidate_incremental_peak': c['allocation_after'][1] - c['allocation_before'][0]})
            result['metadata'].append({'readers': readers, 'pairs': pairs})
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
