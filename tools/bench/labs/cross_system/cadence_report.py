"""Read-only evidence reduction for the registered native cadence sweep.

Execute through the root coordinator/resource launcher. Writes a fresh report;
never reruns native workloads, modifies source evidence, or changes its verdicts.
"""
import argparse
from collections import Counter
import gzip
import hashlib
import json
import math
import os
from pathlib import Path
import statistics
import time

import cadence_sweep as sweep
import prefix_load as shared


def sha_bytes(value):
    return hashlib.sha256(value).hexdigest()


def range_of(values):
    values = list(values)
    return [min(values), max(values)] if values else None


def population_steps(acks, boundary):
    offers = [r['offered_ns'] for r in acks if r['offered_ns'] >= boundary]
    steps = [right - left for left, right in zip(offers, offers[1:])]
    if not steps:
        return {'count': 0, 'median_ns': None, 'fraction_at_or_below_quiet2ms': None}
    return {'count': len(steps), 'median_ns': statistics.median(steps),
            'fraction_at_or_below_quiet2ms': sum(x <= 2_000_000 for x in steps) / len(steps),
            'p99_ns': sorted(steps)[math.ceil(.99 * len(steps)) - 1]}


def reduce_evidence(input_dir):
    receipt = json.loads((input_dir / 'receipt.json').read_text())
    if receipt['state'] != 'complete' or not receipt['scratch_removed'] or len(receipt['cases']) != 54:
        raise RuntimeError('registered sweep did not complete54cells with cleanup')
    cells, chains, negatives, telemetry_controls = [], 0, 0, 0
    hashes = {'receipt.json': shared.sha(input_dir / 'receipt.json')}
    for case in receipt['cases']:
        directory = input_dir / case['label']
        for name in ('metrics.json', 'oracle-verdicts.json', 'telemetry.json.gz', 'success-file-hashes.json'):
            hashes[str((directory / name).relative_to(input_dir))] = shared.sha(directory / name)
        if hashes[str((directory / 'telemetry.json.gz').relative_to(input_dir))] != case['telemetry_sha256']:
            raise RuntimeError('case telemetry no longer matches recorded artifact hash')
        m = json.loads((directory / 'metrics.json').read_text())
        if m != case['metrics']:
            raise RuntimeError('case metric differs from original receipt')
        raw = json.loads(gzip.decompress((directory / 'telemetry.json.gz').read_bytes()))
        sweep.validate_telemetry(raw['result'], raw['acks'], raw['hooks'], raw['timeline'])
        verdicts = json.loads((directory / 'oracle-verdicts.json').read_text())
        positive = [v for v in verdicts if v['label'] not in ('missing', 'duplicate')]
        negative = [v for v in verdicts if v['label'] in ('missing', 'duplicate')]
        if (len(positive) != 8 or any(v['exit'] != 0 or not v['verdict']['passed'] for v in positive)
                or len(negative) != 2 or any(v['exit'] != 1 or v['verdict']['passed'] for v in negative)):
            raise RuntimeError('original independent oracle coverage/outcome mismatch')
        chains += len(positive)
        negatives += len(negative)
        telemetry_controls += len(m['rejected_telemetry_controls'])
        names = Counter(h['name'] for h in raw['hooks'])
        groups = names['frame_append_intent_sync'] if raw['hooks'] else None
        syncs = names['frame_data_or_commit_sync'] if raw['hooks'] else None
        if groups is not None and syncs != 2 * groups:
            raise RuntimeError('selected existing append hooks lack paired data/marker sync coverage')
        cells.append({'label': case['label'], 'shape': m['shape'], 'cadence_ms': m['period_ms'],
            'seed': m['seed'], 'observer': m['observer'], 'offered': m['offered'], 'accepted': m['accepted'],
            'ack_all': m['ack_all'], 'ack_after_sealing': m['ack_offered_after_sealing'],
            'ack_during_sealing': m['ack_offered_during_sealing'],
            'cpu_ns': m['process_cpu_ns'], 'cpu_tick_ns': 1_000_000_000 // m['clk_tck'],
            'whole_process_hwm_bytes': m['whole_process_hwm_includes_startup_bytes'],
            'observer_setup_ns': m['observer_setup_ns'],
            'sealer_duration_ns': m['sealer_end_ns'] - m['sealer_begin_ns'],
            'initial_sizes': m['initial_sizes'],
            'initial_backlog_byte_ns_bounds': m['initial_backlog_byte_ns_bounds'],
            'finite_offered_per_s_from_schedule_start': m['finite_offered_per_s_from_schedule_start'],
            'finite_accepted_per_s_through_last_ack': m['finite_accepted_per_s_through_last_ack'],
            'max_offer_lateness_ns': m['max_offer_lateness_ns'],
            'max_observed_unanswered_offers': m['max_observed_unanswered_offers'],
            'hook_append_groups': groups, 'hook_data_marker_syncs': syncs,
            'mean_accepted_per_hook_append_group': m['accepted'] / groups if groups else None,
            'offer_steps_after_sealing': population_steps(raw['acks'], m['sealer_end_ns']),
            'retained_full_fixture': case['retained_full_fixture'], 'scratch_removed': case['scratch_removed']})
    index = {(c['shape'], c['cadence_ms'], c['seed'], c['observer']): c for c in cells}
    expected = {(s, d, seed, observer) for s in ('balanced3', 'skewed3', 'worker1')
                for d in (0, 2, 5) for seed in sweep.SEEDS for observer in ('quiet', 'events', 'polled')}
    if len(index) != 54 or set(index) != expected:
        raise RuntimeError('duplicate/missing registered cell identities')
    observer_pairs = []
    for shape in ('balanced3', 'skewed3', 'worker1'):
        for cadence in (2, 5):
            for seed in sweep.SEEDS:
                quiet, events, polled = [index[(shape, cadence, seed, o)] for o in ('quiet', 'events', 'polled')]
                observer_pairs.append({'shape': shape, 'cadence_ms': cadence, 'seed': seed,
                    'events_vs_quiet_after_p50_percent': 100 * (events['ack_after_sealing']['p50_ns'] /
                        quiet['ack_after_sealing']['p50_ns'] - 1),
                    'polled_vs_events_after_p50_percent': 100 * (polled['ack_after_sealing']['p50_ns'] /
                        events['ack_after_sealing']['p50_ns'] - 1),
                    'polled_minus_events_cpu_ns': polled['cpu_ns'] - events['cpu_ns'],
                    'quiet_after_p99_ns': quiet['ack_after_sealing']['p99_ns'],
                    'events_after_p99_ns': events['ack_after_sealing']['p99_ns'],
                    'polled_after_p99_ns': polled['ack_after_sealing']['p99_ns']})
    by_cadence = []
    for cadence in (0, 2, 5):
        for observer in ('quiet', 'events', 'polled'):
            rows = [c for c in cells if c['cadence_ms'] == cadence and c['observer'] == observer]
            key = 'ack_all' if cadence == 0 else 'ack_after_sealing'
            by_cadence.append({'cadence_ms': cadence, 'observer': observer, 'ack_population': key,
                'p50_ns_range': range_of(c[key]['p50_ns'] for c in rows),
                'p95_ns_range': range_of(c[key]['p95_ns'] for c in rows),
                'p99_ns_range': range_of(c[key]['p99_ns'] for c in rows),
                'process_cpu_ns_range': range_of(c['cpu_ns'] for c in rows),
                'append_groups_range': range_of(c['hook_append_groups'] for c in rows
                    if c['hook_append_groups'] is not None)})
    return {'source_receipt_sha256': hashes['receipt.json'], 'source_binary_sha256': receipt['binary_sha256'],
        'source_hashes': receipt['source_sha256'], 'artifact_hashes': hashes,
        'source_elapsed_s': receipt['elapsed_s'], 'source_persistent_bytes': receipt['persistent_bytes'],
        'counts': {'cells': len(cells), 'offered': sum(c['offered'] for c in cells),
            'accepted': sum(c['accepted'] for c in cells), 'positive_query_chains': chains,
            'rejected_query_controls': negatives, 'rejected_original_telemetry_controls': telemetry_controls},
        'cells': cells, 'paired_observers': observer_pairs, 'cadence_observer_ranges': by_cadence,
        'limitations': ['Two payload markers are not independently sampled scheduling seeds.',
            '512 ACKs share commit groups; observations are not512 independent durability events.',
            'After-sealing population excludes offers made during sealing, not every possible prior IO/cache effect.',
            'CPU tick resolution is10ms; process HWM includes startup and differs from sampled schedule RSS.',
            'Hook endpoints do not label checkpoint service or identify Segment rename times.',
            'Unobserved/observed debt intervals differ in precision; do not rank unlike bounds as speed gains.',
            'Finite offered/accepted rates are not service-capacity or sustained backpressure qualification.',
            'No historical scheduler baseline or production parameter change was measured.']}


def main():
    if os.environ.get('FABRIC_CROSS_SYSTEM_COORDINATED') != '1':
        raise RuntimeError('root coordinator ownership required')
    shared.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    if args.out.exists():
        raise RuntimeError('fresh report destination required')
    started = time.monotonic()
    try:
        report = reduce_evidence(args.input)
        report.update(state='complete', helper_sha256=shared.sha(Path(__file__)),
                      elapsed_s=time.monotonic() - started)
        args.out.mkdir(parents=True)
        shared.dump(args.out / 'report.json', report)
        print(json.dumps({'state': 'complete', 'report': str(args.out / 'report.json'), 'counts': report['counts']}))
    except BaseException as error:
        args.out.mkdir(parents=True, exist_ok=True)
        shared.dump(args.out / 'failure.json', {'state': 'failed', 'error': repr(error),
            'source_preserved': str(args.input), 'elapsed_s': time.monotonic() - started})
        raise


if __name__ == '__main__':
    main()
