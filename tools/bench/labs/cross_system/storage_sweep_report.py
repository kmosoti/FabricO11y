#!/usr/bin/env python3
"""Derive finite matched storage ratios; coordinator executes under resource_group.

Does not rerun workloads or replace source semantic gates. Refuses incomplete
grids. The JSON retains both seeds, every cost domain and reference variation.
"""
import argparse
import json
from pathlib import Path
import sys

import memory_census as mc

CAPS = (8, 16, 32)
SHAPES = ('steady', 'adversarial', 'bigrows')
TARGETS = (16, 64)
SEEDS = (2703204353, 2703204354)


def read(path, provenance):
    value = json.loads(path.read_text())
    provenance[str(path.resolve())] = mc.digest(path)
    return value


def kib(text):
    return int(text.split()[0])


def ratios(actual, baseline):
    return {key: actual[key]/base if base is not None and base != 0 and actual[key] is not None else None
            for key, base in baseline.items()}


def main():
    mc.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--destination', type=Path, required=True)
    args = parser.parse_args()
    source = args.source.resolve()
    destination = args.destination.resolve()
    if destination.exists() or not destination.is_relative_to(source):
        raise ValueError('fresh report inside source evidence root required')
    provenance = {}
    cells = {}
    reference_rows = {}
    first_sources = None
    for target in TARGETS:
        for repeat, seed in enumerate(SEEDS):
            job = source / f'target-{target}-repeat-{repeat}'
            complete = read(job / 'complete.json', provenance)
            if complete['exit_code'] != 0 or complete['pairs'] != 9 or complete['native_cells'] != 18:
                raise ValueError('storage grid incomplete/failed')
            metadata = read(job / 'metadata.json', provenance)
            if metadata['target_mib'] != target or metadata['repeat'] != repeat or metadata['seed'] != seed:
                raise ValueError('job fixture identity mismatch')
            if first_sources is None:
                first_sources = metadata['source_sha256']
            elif first_sources != metadata['source_sha256']:
                raise ValueError('job frozen source identities differ')
            for shape in SHAPES:
                for cap in CAPS:
                    pair_dir = job / f'run-{cap}-{shape}'
                    pair = read(pair_dir / 'pair.json', provenance)
                    if not all(pair['gates'].values()):
                        raise ValueError('semantic gate failure')
                    if any(pair[key] != value for key, value in {
                            'run_mib': cap, 'shape': shape, 'target_mib': target,
                            'repeat': repeat, 'seed': seed}.items()):
                        raise ValueError('paired fixture identity mismatch')
                    for arm in ('reference', 'bounded'):
                        cell_dir = pair_dir / arm
                        costs = read(cell_dir / 'costs.json', provenance)
                        parent = read(cell_dir / 'parent-before-spawn.json', provenance)
                        filter_check = read(cell_dir / 'physical-filter-check.json', provenance)
                        read(cell_dir / 'complete-member-map.json', provenance)
                        row = pair['rows'][arm]['probe']
                        process = pair['rows'][arm]['process']
                        if process['exit_code'] != 0:
                            raise ValueError('native child failed')
                        io = costs['build_io_delta'] or {}
                        metrics = {
                            'peak_live_heap_bytes': row['peak_live_heap_bytes'],
                            'incremental_peak_heap_bytes': row['incremental_peak_heap_bytes'],
                            'heap_after_build_bytes': row['heap_after_build_bytes'],
                            'build_seconds': row['build_seconds_instrumented'],
                            'build_encoded_mib_per_second': costs['build_encoded_mib_per_second'],
                            'build_journal_mib_per_second': costs['build_journal_mib_per_second'],
                            'whole_native_seconds': process['wall_seconds'],
                            'whole_native_cpu_seconds': process['user_seconds']+process['system_seconds'],
                            'whole_native_peak_rss_kib': process['whole_process_peak_rss_kib'],
                            'build_write_bytes': io.get('write_bytes'),
                            'build_wchar': io.get('wchar'),
                            'build_cancelled_write_bytes': io.get('cancelled_write_bytes'),
                            'build_rchar': io.get('rchar'), 'build_read_bytes': io.get('read_bytes'),
                            'final_segment_bytes': costs['disk_final_logical_bytes']['segment'],
                            'final_journal_bytes': costs['disk_final_logical_bytes']['journal']}
                        parent_hwm = kib(parent['process']['VmHWM'])
                        entry = {'target_mib': target, 'shape': shape, 'run_mib': cap,
                                 'seed': seed, 'repeat': repeat, 'arm': arm, 'metrics': metrics,
                                 'input_sha256': row['input_sha256'], 'ordered_row_ledgers': row['ordered_row_ledgers'],
                                 'manifest': row['manifest'],
                                 'parent_hwm_kib': parent_hwm,
                                 'rss_parent_floor_below_worker': (not parent['pyarrow_imported'] and
                                     parent_hwm < metrics['whole_native_peak_rss_kib']),
                                 'spill_count_lower_bounds': costs['level0_spill_count_lower_bound'],
                                 'intermediate_merge_observed': costs['intermediate_merge_observed'],
                                 'filter_groups': {name: {'physical_groups': value['parquet_groups'],
                                                       'logical_rows': value['logical_rows']}
                                                   for name, value in filter_check['filters'].items()}}
                        cells[target, shape, seed, cap, arm] = entry
                        if arm == 'reference':
                            reference_rows[target, shape, seed, cap] = row
    bounded = []
    reference_variation = []
    for target in TARGETS:
        for shape in SHAPES:
            for seed in SEEDS:
                base = cells[target, shape, seed, 16, 'bounded']
                ref_base = cells[target, shape, seed, 16, 'reference']
                for cap in CAPS:
                    entry = cells[target, shape, seed, cap, 'bounded']
                    ref = cells[target, shape, seed, cap, 'reference']
                    ratio = ratios(entry['metrics'], base['metrics'])
                    if entry['input_sha256'] != base['input_sha256'] or entry['ordered_row_ledgers'] != base['ordered_row_ledgers']:
                        raise ValueError('cross-cap logical fixture/result divergence')
                    useful_change = (ratio['peak_live_heap_bytes'] <= .9 or
                        entry['spill_count_lower_bounds'] != base['spill_count_lower_bounds'] or
                        entry['intermediate_merge_observed'] != base['intermediate_merge_observed'])
                    benefit = ratio['peak_live_heap_bytes'] <= .9 or (
                        ratio['build_write_bytes'] is not None and ratio['build_write_bytes'] <= .9)
                    guards = {'benefit_at_least_10_percent': benefit,
                              'throughput_at_least_90_percent': ratio['build_encoded_mib_per_second'] >= .9,
                              'native_cpu_at_most_110_percent': ratio['whole_native_cpu_seconds'] <= 1.1,
                              'native_rss_at_most_110_percent': ratio['whole_native_peak_rss_kib'] <= 1.1,
                              'useful_work_observation_differs': useful_change,
                              'rss_parent_floor_below_worker': entry['rss_parent_floor_below_worker'] and base['rss_parent_floor_below_worker']}
                    bounded.append({**entry, 'ratios_to_bounded_16': ratio,
                                    'screen_guards': guards, 'finite_seed_screen': all(guards.values()),
                                    'causal_limit': 'sampled spill counts are lower bounds; difference need not prove exact cap binding'})
                    ref_gates = mc.grade(reference_rows[target, shape, seed, cap], reference_rows[target, shape, seed, 16])
                    reference_variation.append({'target_mib': target, 'shape': shape, 'seed': seed,
                            'run_mib': cap, 'metrics': ref['metrics'],
                            'ratios_to_reference_16': ratios(ref['metrics'], ref_base['metrics']),
                            'cross_cap_reference_semantic_gates': ref_gates})
                    if not all(ref_gates.values()):
                        raise ValueError('cross-cap reference semantic divergence')
    screens = []
    for target in TARGETS:
        for shape in SHAPES:
            for cap in (8, 32):
                rows = [row for row in bounded if row['target_mib'] == target and row['shape'] == shape and row['run_mib'] == cap]
                screens.append({'target_mib': target, 'shape': shape, 'run_mib': cap,
                                'seed_outcomes': {str(row['seed']): row['finite_seed_screen'] for row in rows},
                                'both_seeds_screen': len(rows) == 2 and all(row['finite_seed_screen'] for row in rows),
                                'promotion': 'unsupported; separate fresh confirmation required'})
    result = {'scope': '36 matched pairs, 72 native cells; existing semantic gates',
              'bounded_cells': bounded, 'reference_control_variation': reference_variation,
              'screens': screens, 'input_receipt_sha256': provenance,
              'reporter_sha256': mc.digest(Path(__file__)), 'qualification': False,
              'source_sha256': first_sources,
              'limitations': ['two seeds per shape/size; no uncertainty interval or fresh holdout',
                              'same Rust row scanners differential; independent decoder checks only FTF1 construction',
                              'no reservation metric or exclusive phase counts',
                              'IO counters approximate kernel work, not completed device amplification',
                              '100ms filesystem observations can miss short-lived spill files',
                              'warm cache/order shared; no controlled cold/warm claim']}
    mc.dump(destination, result)
    print(json.dumps({'report': str(destination), 'bounded_cells': len(bounded),
                      'both_seeds_screens': [row for row in screens if row['both_seeds_screen']]}))


if __name__ == '__main__':
    main()
