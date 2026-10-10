#!/usr/bin/env python3
"""Deterministic planning arithmetic; this does not run a deployment benchmark."""
import argparse
import hashlib
import json
import math
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
DEFAULT_PAIRS = ROOT / 'docs/experiments/benchmarks/data/streaming-output-local-run-01/pairs.json'
MIB = 2**20
GIB = 2**30
DAY = 86400


def positive(value, name):
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value <= 0:
        raise ValueError(f'{name} must be a positive finite number')


def calculate(config, pairs, evidence_hash):
    a = config['assumptions']
    for key, value in a.items():
        if key != 'storage_ratios':
            positive(value, key)
    for ratio in a['storage_ratios']:
        positive(ratio, 'storage ratio')
    if not a['storage_ratios']:
        raise ValueError('at least one storage ratio required')
    if a['service_derating'] > 1 or a['memory_target_fraction'] > 1 or a['burst_multiplier'] < 1:
        raise ValueError('derating/memory fraction must be <=1; burst multiplier must be >=1')
    if a['file_mib'] != 64:
        raise ValueError('only 64 MiB file service calibration is available')
    # Use the slowest baseline 64 MiB entropy trial, not the experimental candidate.
    rows = [p['baseline'] for p in pairs if p['shape'] == 'entropy' and p['mib'] == 64]
    if not rows:
        raise ValueError('no matching baseline calibration trials')
    slowest = min(x['encoded_group_bytes'] / x['build_seconds_instrumented'] for x in rows)
    service = slowest * a['service_derating']
    results = []
    for s in config['scenarios']:
        for key in ('nodes', 'events_per_second', 'retention_days', 'trial_vcpus', 'trial_ram_gib'):
            positive(s[key], key)
        avg = s['events_per_second'] * a['encoded_bytes_per_event']
        peak = avg * a['burst_multiplier']
        average_workers = max(1, math.ceil(avg / service))
        peak_workers = max(1, math.ceil(peak / service))
        # Fluid approximation: work queue drains at service minus continuing average ingress.
        backlog = max(0, peak - average_workers * service) * a['burst_seconds']
        drain = backlog / (average_workers * service - avg)
        journal = max(a['journal_floor_gib'] * GIB, peak * a['outage_seconds'])
        server_gib = a['fixed_server_gib'] + peak_workers * a['builder_gib'] * a['builder_margin']
        disks = []
        for c in a['storage_ratios']:
            retained = avg * DAY * s['retention_days'] * c
            temporary = peak_workers * a['file_mib'] * MIB * (1 + c)
            disks.append({
                'stored_bytes_per_input_byte': c,
                'retained_gib': retained / GIB,
                'state_device_gib': (retained + journal + temporary) * a['disk_margin'] / GIB,
                'disk_write_mib_s_at_peak': peak * (1 + c) / MIB,
                'disk_read_mib_s_at_peak': peak / MIB,
                'default_20gib_retention_hours': 20 * GIB / (avg * c) / 3600,
            })
        results.append({
            **s, 'average_mib_s': avg / MIB, 'peak_mib_s': peak / MIB,
            'encoded_input_gib_per_day': avg * DAY / GIB,
            'peak_network_mbit_s_with_margin': peak * 8 * a['network_margin'] / 1e6,
            'average_workers_model': average_workers, 'peak_workers_model': peak_workers,
            'peak_sealing_utilization_model': peak / (peak_workers * service),
            'burst_backlog_gib_with_average_workers': backlog / GIB,
            'post_burst_drain_seconds_model': drain,
            'journal_budget_gib': journal / GIB,
            'server_memory_allowance_gib': server_gib,
            'host_memory_floor_gib_model': server_gib / a['memory_target_fraction'] + a['host_reserve_gib'],
            'exceeds_default_server_3gib_limit': server_gib > 3,
            'workers_exceed_config_max_16': peak_workers > 16,
            'per_node_one_hour_spool_mib_average': avg * a['outage_seconds'] / s['nodes'] / MIB,
            'per_node_one_hour_spool_mib_peak': peak * a['outage_seconds'] / s['nodes'] / MIB,
            'storage': disks,
        })
    return {
        'status': 'planning estimates; no new deployment performance measurements',
        'evidence_sha256': evidence_hash,
        'calibration': {'baseline_slowest_64mib_entropy_build_mib_s': slowest / MIB,
                        'derated_worker_service_mib_s': service / MIB,
                        'boundary': 'instrumented single Segment build with resident input; excludes live ingest and journal replay'},
        'assumptions': a, 'results': results,
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--scenarios', type=Path, default=Path(__file__).with_name('scenarios.json'))
    ap.add_argument('--pairs', type=Path, default=DEFAULT_PAIRS)
    args = ap.parse_args()
    raw = args.pairs.read_bytes()
    try:
        result = calculate(json.loads(args.scenarios.read_text()), json.loads(raw), hashlib.sha256(raw).hexdigest())
    except (ValueError, KeyError, ZeroDivisionError) as e:
        ap.error(str(e))
    print(json.dumps(result, indent=2, allow_nan=False))


if __name__ == '__main__':
    main()
