#!/usr/bin/env python3
"""Derive scoped evidence summaries; coordinator runs under resource launcher."""
import argparse
import json
from pathlib import Path


def stat_cpu(value):
    if value is None:
        return None
    # comm is parenthesized and may contain spaces or ')'; fields after the
    # final ')' start at field3 (state). utime/stime are fields14/15.
    fields = value.rsplit(')', 1)[1].split()
    return {'user_ticks': int(fields[11]), 'system_ticks': int(fields[12])}


def io(value):
    if value is None:
        return None
    return {key: int(number) for key, number in
            (line.split(':', 1) for line in value.splitlines())}


def delta(before, after):
    if before is None or after is None:
        return None
    return {key: after[key] - before[key] for key in before.keys() & after.keys()}


def measure(row, ticks_per_second):
    ticks = delta(stat_cpu(row.get('stat_before')), stat_cpu(row.get('stat_after')))
    seconds = None if ticks is None or ticks_per_second is None else sum(ticks.values()) / ticks_per_second
    build_wall = row['build_seconds_instrumented']
    return {
        'incremental_builder_heap_mib': row['incremental_peak_heap_bytes'] / 1024**2,
        'starting_live_heap_mib': row['heap_start_bytes'] / 1024**2,
        'heap_after_build_mib': row['heap_after_build_bytes'] / 1024**2,
        'build_wall_seconds_instrumented': build_wall,
        'build_boundary_cpu_ticks': ticks,
        'build_boundary_cpu_seconds': seconds,
        'build_boundary_cpu_core_equivalents': None if seconds is None else seconds / build_wall,
        'build_boundary_proc_io_delta': delta(io(row.get('io_before')), io(row.get('io_after'))),
        'whole_process_peak_rss_mib': row['whole_process_peak_rss_kib'] / 1024,
        'whole_process_user_seconds': row['user_seconds'],
        'whole_process_system_seconds': row['system_seconds'],
        'whole_process_wall_seconds': row['process_wall_seconds'],
        'journal_mib': row['journal_bytes'] / 1024**2,
        'encoded_group_mib': row['encoded_group_bytes'] / 1024**2,
        'table_rows': {name: entry['rows'] for name, entry in row['manifest']['files'].items() if name.endswith('.parquet')},
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('campaigns', nargs='+', type=Path)
    ap.add_argument('--ticks-per-second', type=int)
    ap.add_argument('--output', required=True, type=Path)
    args = ap.parse_args()
    if args.output.exists(): ap.error('output must not exist')
    if args.ticks_per_second is not None and args.ticks_per_second <= 0: ap.error('tick frequency must be positive')
    cells = []
    for campaign in args.campaigns:
        completion = campaign / 'complete.json'
        for path in sorted(campaign.glob('*/pair.json')):
            pair = json.loads(path.read_text())
            reference = measure(pair['reference'], args.ticks_per_second)
            bounded = measure(pair['bounded'], args.ticks_per_second)
            heap_ratio = bounded['incremental_builder_heap_mib'] / reference['incremental_builder_heap_mib']
            cells.append({'pair_artifact': str(path), 'campaign_completed': completion.exists(),
                          'campaign_state': 'failed' if (campaign / 'failure.json').exists() else ('completed' if completion.exists() else 'incomplete'),
                          'shape': pair['shape'], 'mib': pair['mib'], 'order': pair['order'],
                          'gates': pair['gates'], 'reference': reference, 'bounded': bounded,
                          'bounded_heap_percent_of_reference': 100 * heap_ratio,
                          'heap_reduction_percent': 100 * (1 - heap_ratio),
                          'bounded_wall_ratio': bounded['build_wall_seconds_instrumented'] / reference['build_wall_seconds_instrumented']})
    value = {'cells': cells, 'original_screen_failed': any(c['campaign_state'] == 'failed' for c in cells),
             'ticks_per_second': args.ticks_per_second,
             'tick_frequency_assumption': 'supplied by coordinator from actual measurement host; otherwise CPU ticks remain unconverted',
             'limitations': [
                 'one pair per reconstructed shape; no statistical performance inference or full BS acceptance',
                 'counting allocator instruments both builders and timing',
                 'whole-process RSS and CPU include fixture generation plus verification; build metrics are separate',
                 '/proc CPU/IO boundaries include small sensor reads; wall timer covers only read/build',
                 'rchar/wchar are syscall character counts; read_bytes/write_bytes are storage accounting; neither isolates spill bytes',
                 'cgroup anon/file/kernel/peak/events belong to coordinator receipts, not these process counters',
                 'ratio is a descriptive paired screen; runtime comparison is not an acceptance gate']}
    args.output.write_text(json.dumps(value, indent=2) + '\n')


if __name__ == '__main__':
    main()
