#!/usr/bin/env python3
"""Descriptive aggregation of unchanged per-trial revision-2 evidence."""
import argparse
import json
import statistics
from pathlib import Path


def distribution(values):
    return {'n': len(values), 'median': statistics.median(values), 'min': min(values),
            'max': max(values), 'cv': statistics.stdev(values) / statistics.mean(values)
            if len(values) > 1 and statistics.mean(values) else None}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('run', type=Path)
    ap.add_argument('output', type=Path)
    a = ap.parse_args()
    data = json.loads((a.run / 'summary.json').read_text())
    out = {'definition': 'Median of per-trial medians; total costs also aggregated separately. No pooled p99. Heap from counted variants; timing/CPU from plain variants.', 'cases': {}}
    for case in dict.fromkeys(x['case_id'] for x in data):
        plain = [x for x in data if x['case_id'] == case and x['variant'] == 'plain']
        counted = [x for x in data if x['case_id'] == case and x['variant'] == 'counted']
        stages = {}
        for name in plain[0]['stats']:
            p = [x['stats'][name] for x in plain]
            c = [x['stats'][name] for x in counted]
            wall = distribution([x['wall_ms']['p50'] for x in p])
            stages[name] = {'wall_p50_ms': wall, 'unstable': wall['cv'] > .2,
                            'wall_sum_ms': distribution([x['wall_ms']['sum'] for x in p]),
                            'cpu_sum_ms': distribution([x['cpu_ns'] / 1e6 for x in p]),
                            'units': [x['units'] for x in p],
                            'samples': [x['wall_ms']['samples'] for x in p],
                            'heap_baseline_mib': distribution([x['max_baseline_live_bytes'] / 2**20 for x in c]),
                            'heap_incremental_peak_mib': distribution([x['max_incremental_peak_bytes'] / 2**20 for x in c]),
                            'heap_requested_mib': distribution([x['cumulative_requested_bytes'] / 2**20 for x in c]),
                            'counted_wall_p50_ms': distribution([x['wall_ms']['p50'] for x in c])}
        out['cases'][case] = {'records': plain[0]['records'], 'body_bytes': plain[0]['fixture']['body_bytes'],
                              'body_inventory_mib': plain[0]['records'] * plain[0]['fixture']['body_bytes'] / 2**20,
                              'encoded_batch_bytes': plain[0]['fixture']['encoded_batch_bytes'],
                              'process_hwm_mib': distribution([x['complete']['vm_hwm_kib'] / 1024 for x in plain]),
                              'stages': stages}
    a.output.write_text(json.dumps(out, indent=2) + '\n')


if __name__ == '__main__':
    main()
