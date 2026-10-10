"""Summarize retained continuation evidence without changing any verdict."""
import argparse
import json
from pathlib import Path
import statistics

BASE = Path('docs/experiments/benchmarks/data/cross-system-run-01')


def read(path):
    return json.loads(path.read_text())


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    result = {'scope': 'descriptive finite samples; no inferential significance or qualification'}
    result['jobs'] = [{key: row.get(key) for key in ('id', 'state', 'exit', 'elapsed_s', 'scratch_removed')}
                      for path in sorted((BASE / 'coordinator').glob('*/receipt.json'))
                      if (row := read(path))]
    result['memory'] = {}
    for run in ('census-01', 'census-02', 'census-03'):
        root = BASE / 'memory' / run
        if not root.exists():
            continue
        rows = []
        for path in sorted(root.glob('*/pair.json')):
            pair = read(path)
            values = {}
            for arm in ('reference', 'bounded'):
                probe = pair['rows'][arm]['probe']
                values[arm] = {key: probe[key] for key in
                    ('incremental_peak_heap_bytes', 'build_seconds_instrumented', 'journal_bytes')}
                values[arm]['process'] = read(path.parent / arm / 'process.json')
            rows.append({'shape': pair['shape'], 'gates': pair['gates'], 'values': values})
        result['memory'][run] = {'completed': (root / 'complete.json').exists(), 'pairs': rows}
    result['operations'] = {}
    for root in sorted((BASE / 'operations').glob('prefix-load-*')):
        if not (root / 'receipt.json').exists():
            continue
        receipt = read(root / 'receipt.json')
        pairs = []
        for shape in ('balanced3', 'skewed3', 'worker1'):
            for seed in (2703204353, 2703204354, 2703204355):
                files = {arm: root / f'{shape}-{seed}-{arm}/metrics.json'
                         for arm in ('baseline', 'candidate')}
                if not all(path.exists() for path in files.values()):
                    continue
                values = {arm: read(path) for arm, path in files.items()}
                b, c = [values[arm]['initial_backlog_byte_ns_bounds'] for arm in files]
                savings = [1-c[1]/b[0], 1-c[0]/b[1]]
                selected = {arm: {key: row[key] for key in
                    ('accepted', 'offered', 'accepted_bytes', 'ack_p99_ns', 'process_cpu_ticks',
                     'clk_tck', 'sampled_schedule_rss_peak_bytes', 'schedule_end_ns', 'sealer_end_ns',
                     'initial_backlog_byte_ns_bounds', 'natural_skew_observed', 'poll_ms',
                     'exact_batch_custody', 'restart_retry')}
                    for arm, row in values.items()}
                for row in selected.values():
                    row['accepted_batches_per_schedule_second'] = row['accepted'] * 1e9 / row['schedule_end_ns']
                    row['accepted_bytes_per_schedule_second'] = row['accepted_bytes'] * 1e9 / row['schedule_end_ns']
                pairs.append({'shape': shape, 'seed': seed, 'journal_byte_time_savings_bounds': savings,
                              'ack_p99_change': values['candidate']['ack_p99_ns']/values['baseline']['ack_p99_ns']-1,
                              'arms': selected})
        result['operations'][root.name] = {'state': receipt['state'], 'cases': len(receipt['cases']), 'pairs': pairs}
    result['query'] = {}
    for root in sorted((BASE / 'query').glob('census-*')):
        if not (root / 'results.json').exists():
            continue
        rows = read(root / 'results.json')
        result['query'][root.name] = {
            'cells': len(rows), 'chains': sum(r['metrics']['complete_chains'] for r in rows),
            'pages': sum(r['metrics']['complete_chain_pages'] for r in rows),
            'resources': {r['cell']: r['metrics']['whole_probe_resources'] for r in rows}}
        verdicts = [read(p) for p in root.glob('n*/oracle.json')]
        result['query'][root.name]['accepted_verdicts'] = sum(
            v['verdict']['passed'] for item in verdicts for v in item['verdicts'])
        result['query'][root.name]['rejected_controls'] = sum(
            not v['passed'] for item in verdicts for v in item['controls'].values())
        if (root / 'borrowed-allocation-pairs.json').exists():
            by_cell = {(r['records'], r['body_bytes'], r['variant']): r for r in rows}
            ratios = []
            for n, width in ((128,16), (128,1024), (2048,16), (2048,1024)):
                a = by_cell[n,width,'plain-owned']['metrics']['first_page_measurements']
                b = by_cell[n,width,'plain-borrowed']['metrics']['first_page_measurements']
                for stage in sorted({r['stage'] for r in a if r['stage'].startswith('query_')}):
                    owned = [r for r in a if r['stage'] == stage]
                    borrowed = [r for r in b if r['stage'] == stage]
                    item = {'records': n, 'body_bytes': width, 'stage': stage,
                            'samples_per_arm': len(owned)}
                    for metric in ('cpu_ns', 'wall_ns'):
                        baseline = statistics.median(r[metric] for r in owned)
                        candidate = statistics.median(r[metric] for r in borrowed)
                        item[metric] = {'owned_median': baseline, 'borrowed_median': candidate,
                                        'borrowed_over_owned': candidate/baseline}
                    ratios.append(item)
            result['query'][root.name]['plain_population_ratios'] = ratios
            allocation = read(root / 'borrowed-allocation-pairs.json')
            result['query'][root.name]['target_allocation_calls'] = [r for r in allocation['paired_counted_calls']
                if r['records'] == 2048 and r['body_bytes'] == 1024 and r['stage'].startswith(
                    ('query_segment_scan_selective_', 'query_segment_walk_selective_'))]
    args.out.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'report': str(args.out), 'jobs': len(result['jobs']),
                      'memory': list(result['memory']), 'operations': list(result['operations']),
                      'query': list(result['query'])}))


if __name__ == '__main__':
    main()
