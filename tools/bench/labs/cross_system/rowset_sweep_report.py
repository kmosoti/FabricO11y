#!/usr/bin/env python3
"""Finite RowSet screen reducer; coordinator execution only; no global winner."""
import argparse
import json
from pathlib import Path

import memory_census as mc

PHASES = ('construction', 'and_once', 'or_once', 'top64_once', 'and_batch', 'or_batch',
          'top64_batch', 'serialization', 'conversion_and', 'conversion_or')


def ratio(actual, base):
    return actual/base if base else None


def main():
    mc.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--destination', type=Path, required=True)
    args = parser.parse_args()
    if args.destination.exists() or not args.destination.resolve().is_relative_to(args.source.resolve()):
        raise ValueError('fresh report inside RowSet evidence root required')
    complete = json.loads((args.source/'complete.json').read_text())
    controls = json.loads((args.source/'negative-controls.json').read_text())
    if complete['exit_code'] != 0 or complete['native_cells'] != 48 or not complete['scratch_removed']:
        raise ValueError('RowSet grid incomplete or not cleaned')
    if not controls['valid_accepted'] or set(controls['rejected']) != {'missing', 'extra', 'order'}:
        raise ValueError('exact-membership controls did not reject representative mutations')
    if not controls.get('known_license_alias_target_validated') or set(controls.get('license_alias_controls_rejected', [])) != {
            'wrong_link', 'wrong_path', 'absolute_link', 'wrong_package', 'absent_target'}:
        raise ValueError('registered license alias controls incomplete')
    summary = json.loads((args.source/'summary.json').read_text())
    cells = {}
    for row in summary['cells']:
        key = row['universe'], row['shape'], row['seed'], row['arm']
        if key in cells or not row['oracle_exact'] or row['process']['exit_code'] != 0:
            raise ValueError('duplicate/failed/non-oracle cell')
        if row['metrics']['representation'] != row['arm'] or row['metrics']['universe'] != row['universe']:
            raise ValueError('native cell identity mismatch')
        if row['metrics']['batch_iterations'] != 128:
            raise ValueError('batch iteration mismatch')
        cells[key] = row
    expected = {(n, shape, seed, arm) for n in (32768, 262144)
                for shape in ('sparse', 'spread', 'clustered', 'common')
                for seed in (2703204353, 2703204354) for arm in ('vector', 'dense', 'roaring')}
    if set(cells) != expected:
        raise ValueError('finite grid coverage differs')
    rows = []
    for key in sorted(cells):
        n, shape, seed, arm = key
        cell = cells[key]
        actual = cell['metrics']
        base_cell = cells[n, shape, seed, 'vector']
        base = base_cell['metrics']
        for field in ('input_a_cardinality', 'input_b_cardinality', 'and_cardinality', 'or_cardinality'):
            if actual[field] != base[field]:
                raise ValueError('matched representation cardinalities diverged')
        phases = {phase: {'absolute': actual[phase], 'ratios_to_vector': {
            field: ratio(value, base[phase][field]) for field, value in actual[phase].items()}}
                  for phase in PHASES}
        amortized = {phase: {'per_call_wall_ns': actual[phase]['wall_ns']/128,
                            'per_call_requested_bytes': actual[phase]['requested_bytes']/128,
                            'batch_peak_incremental_bytes': actual[phase]['peak_incremental_bytes']}
                     for phase in ('and_batch', 'or_batch', 'top64_batch')}
        models = {}
        for lifetime in (1, 128):
            and_name = 'and_once' if lifetime == 1 else 'and_batch'
            top_name = 'top64_once' if lifetime == 1 else 'top64_batch'
            wanted = actual['construction']['wall_ns']+actual[and_name]['wall_ns']+actual[top_name]['wall_ns']
            baseline = base['construction']['wall_ns']+base[and_name]['wall_ns']+base[top_name]['wall_ns']
            models[str(lifetime)] = {'phase_sum_wall_ns': wanted, 'ratio_to_vector_phase_sum': ratio(wanted, baseline),
                                    'definition': 'construction + AND + top64 of intersection; excludes serialization/full conversion',
                                    'measured_continuous_pipeline': False}
        rows.append({'universe': n, 'shape': shape, 'seed': seed, 'arm': arm,
                     'phases': phases, 'amortized_operations': amortized,
                     'constructed_input_pair_bytes': actual['construction']['retained_incremental_bytes'],
                     'native_serialized_bytes': actual['native_serialized_bytes'],
                     'serialized_ratio_to_vector': ratio(actual['native_serialized_bytes'], base['native_serialized_bytes']),
                     'native_postexec_hwm_kib': actual['native_postexec_hwm_kib'],
                     'native_hwm_ratio_to_vector': ratio(actual['native_postexec_hwm_kib'], base['native_postexec_hwm_kib']),
                     'whole_native_cpu_seconds': cell['process']['user_seconds']+cell['process']['system_seconds'],
                     'whole_native_wall_seconds': cell['process']['wall_seconds'],
                     'wait4_rss_kib_uninterpreted': cell['process']['wait4_rss_kib'],
                     'cardinalities': {field: actual[field] for field in ('input_a_cardinality', 'input_b_cardinality',
                                                                        'and_cardinality', 'or_cardinality')},
                     'construction_amortization_models': models, 'oracle_exact': True})
    result = {'cells': rows, 'qualification': False, 'controls': controls,
              'input_receipt_sha256': {name: mc.digest(args.source/name)
                  for name in ('summary.json', 'complete.json', 'negative-controls.json', 'metadata.json')},
              'reporter_sha256': mc.digest(Path(__file__)),
              'baseline_limitation': 'sorted Vec two-pointer AND/OR starts Vec::new and grows; no reserve/galloping; does not establish optimal Vec baseline',
              'roaring_construction': 'sorted input collection + optimize cost included; no claim default-only construction',
              'limitations': ['counted allocator perturbation; no paired plain binary',
                              'top64 times an already materialized intersection, not fused AND/top64',
                              'phase sums are models, not continuous pipeline measurements',
                              'two fresh deterministic seeds; batches reuse inputs',
                              'native HWM includes fixture decoding, correctness export and conversion',
                              'wait4 RSS can contain Python parent pre-exec floor',
                              'native serialization roundtrip uses same library; independent Python checks canonical membership only'],
              'research_interpretation': 'may motivate cardinality/locality/lifetime-dependent dispatch; no global Roaring superiority or Fabric production claim'}
    mc.dump(args.destination, result)
    print(json.dumps({'report': str(args.destination), 'cells': len(rows), 'global_winner': 'not claimed'}))


if __name__ == '__main__':
    main()
