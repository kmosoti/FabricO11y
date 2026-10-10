#!/usr/bin/env python3
"""Independent archived accounting audit; never edits measured evidence.

Scan/Walk call the unchanged check_dev_small_observations.checks implementation
through lossless scratch adapters. Off checks below adapt its custody/rate/CPU
logic, explicitly omitting unmeasured query/visibility populations. Supplemental
phase CPU, RSS and query medians are recomputed here without Observer helpers.
"""
import argparse
from collections import Counter
import copy
import gzip
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[5]
sys.path.insert(0, str(ROOT / 'tools/bench'))
import check_dev_small_observations as independent
from resource_group import STORAGE, require_limits

DATA = ROOT / 'docs/experiments/benchmarks/data/readiness-labs-run-01/query'


def clock_range(offsets_ns, declared_range_ms):
    """Subtract integer offsets before conversion; tolerate only float encoding."""
    raw = (max(offsets_ns) - min(offsets_ns)) / 1_000_000
    largest_offset_ms = max(abs(value) for value in offsets_ns) / 1e6
    tolerance = 2 * math.ulp(largest_offset_ms)
    delta = abs(raw - declared_range_ms)
    return {'raw_integer_range_ms': raw, 'declared_range_ms': declared_range_ms,
            'absolute_delta_ms': delta, 'representation_tolerance_ms': tolerance,
            'accounting_consistent': delta <= tolerance,
            'integer_range_le_5ms': raw <= 5,
            'note': 'representation tolerance applies only to accounting equality; 5ms gate is unchanged'}


def off_checks(root, summary):
    """Scoped adaptation of independent checker; no dummy visibility/query rows."""
    problems = []
    def require(ok, reason):
        if not ok:
            problems.append(reason)
    sources = independent.lines(root / 'sources.jsonl.gz')
    sizes = {(r[0], r[1]): r[3] for r in independent.lines(root / 'recovered-hashes.jsonl.gz')}
    events = [(p.name.split('-')[0], r) for p in root.glob('node*-events.jsonl.gz')
              for r in independent.lines(p)]
    cycles = {(n, int(r['batch'])): r for n, r in events if r['kind'] == 'cycle'}
    acks = {(n, int(r['sequence'])): r for n, r in events
            if r['kind'] == 'ack_attempt' and r['status'] == 'ack'}
    require(set(cycles) == set(acks) == set(sizes), 'cycle/ACK/recovery census differs')
    require(sum(int(r['logs']) for r in cycles.values()) == len(sources) == summary['recovered_logs']
            == summary['expected_logs'], 'source/cycle/recovery log totals differ')
    total = Counter()
    for row in independent.read(root / 'rates-per-second.json'):
        total.update({k: v for k, v in row.items() if k != 'second'})
    for field, expected in [('spool_batches', len(cycles)), ('spool_bytes', sum(sizes.values())),
                            ('unique_acks', len(acks)), ('ack_bytes', sum(sizes.values())),
                            ('source_logs', len(sources)), ('source_bytes', len(sources) * 901)]:
        require(total[field] == expected, 'bucket census ' + field)
    epoch = summary['observation']['epoch_ns']
    for phase, offset in [('normal', 0), ('burst', 60), ('recovery', 120)]:
        lo, hi = epoch + offset * 10**9, epoch + (offset + 60) * 10**9
        expected = Counter()
        for node, row in events:
            if lo <= row['t'] < hi:
                if row['kind'] == 'cycle':
                    expected['spool_batches'] += 1
                    expected['spool_bytes'] += sizes[(node, int(row['batch']))]
                else:
                    expected['send_attempts'] += 1
                    expected['status_' + row['status']] += 1
        expected['source_logs'] = sum(lo <= r[2] < hi for r in sources)
        actual = summary['observation']['rates'][phase]
        for key, value in expected.items():
            require(actual['counts'].get(key, 0) == value, phase + ' count ' + key)
            require(math.isclose(actual['per_second'].get(key, 0), value / 60, abs_tol=1e-12),
                    phase + ' rate ' + key)
    resources = independent.read(root / 'resources.json')
    seconds = (resources[-1]['mono_ns'] - resources[0]['mono_ns']) / 10**9
    cpu = resources[-1]['processes'][0]['cpu_s'] - resources[0]['processes'][0]['cpu_s']
    require(math.isclose(summary['observation']['cpu']['whole']['processes']['server']['mean_cores'],
                         cpu / seconds, abs_tol=1e-12), 'server CPU mean')
    require(summary['observation']['cgroup_peak_bytes'] == max(int(r['cgroup']['memory.peak'])
            for r in resources), 'cgroup peak')
    require(not independent.lines(root / 'queries.jsonl.gz'), 'off timed queries present')
    require(not independent.read(root / 'visibility.json'), 'off visibility targets present')
    for name in ('queries', 'visibility'):
        require(summary['observation'][name]['status'] == 'not_applicable' and
                summary['observation'][name]['value'] is None, 'off ' + name + ' unmeasured marker')
    return problems


def supplemental(root, summary, cell):
    problems = []
    limitations = []
    def require(ok, reason):
        if not ok:
            problems.append(reason)
    samples = independent.read(root / 'resources.json')
    epoch = summary['observation']['epoch_ns']
    offsets = [r['wall_ns'] - r['mono_ns'] for r in samples]
    clock = clock_range(offsets, summary['clock_offset_range_ms'])
    require(clock['accounting_consistent'],
            'clock-offset range raw recomputation')
    clock_valid = summary['gates']['clock_offset_range_le_5ms']
    if not clock_valid:
        limitations.append('clock-discontinuous wall phases cannot support a matched performance comparison')
    cores, rss = [], []
    for phase, offset in [('normal', 0), ('burst', 60), ('recovery', 120)]:
        rows = [r for r in samples if epoch + offset * 10**9 <= r['wall_ns'] < epoch + (offset + 60) * 10**9]
        if len(rows) < 2:
            require(summary['observation']['cpu'][phase]['samples'] == len(rows), phase + ' absent CPU sample census')
            limitations.append(phase + ' CPU population unavailable: ' + str(len(rows)) + ' samples')
            continue
        seconds = (rows[-1]['mono_ns'] - rows[0]['mono_ns']) / 10**9
        cpu = (rows[-1]['processes'][0]['cpu_s'] - rows[0]['processes'][0]['cpu_s']) / seconds
        cores.append(cpu)
        rss.extend(r['processes'][0]['rss_kib'] / 1024 for r in rows)
        require(math.isclose(cpu, summary['observation']['cpu'][phase]['processes']['server']['mean_cores'],
                             abs_tol=1e-12), phase + ' CPU mean')
    metrics = summary.get('lab', {}).get('metrics')
    if metrics is None:
        limitations.append('original failed annotation: balanced metrics absent; original evidence preserved')
    elif clock_valid:
        require(len(cores) == 3 and math.isclose(sum(cores) / 3, metrics['balanced_phase_server_cores'], abs_tol=1e-12),
                'balanced phase CPU')
        require(bool(rss) and max(rss) == metrics['offered_phase_peak_server_rss_mib'], 'offered phase RSS')
    if cell == 'off':
        require(metrics['balanced_phase_shape_median_ms'] is None, 'off missing latency must stay null')
    else:
        queries = independent.lines(root / 'queries.jsonl.gz')
        medians = []
        for phase, offset in [('normal', 0), ('burst', 60), ('recovery', 120)]:
            for shape in ('recent_logs', 'absent_text', 'cpu_metrics'):
                population = sorted(q['elapsed_ms'] for q in queries if q['shape'] == shape and
                    epoch + offset * 10**9 <= q['start_ns'] < epoch + (offset + 60) * 10**9)
                require(bool(population), phase + ' missing query population ' + shape)
                if population:
                    median = population[math.ceil(len(population) * .5) - 1]
                    medians.append(median)
                    require(median == summary['observation']['queries'][shape][phase]['latency_ms']['p50'],
                            phase + ' query median ' + shape)
                else:
                    # Absence remains explicit. A clock-invalid run has no
                    # matched latency population; this is not an accounting lie.
                    if not clock_valid:
                        problems.remove(phase + ' missing query population ' + shape)
                    limitations.append(phase + ' missing query population ' + shape)
                    require(summary['observation']['queries'][shape][phase]['latency_ms']['samples'] == 0 and
                            summary['observation']['queries'][shape][phase]['latency_ms']['p50'] is None,
                            phase + ' empty query population marker ' + shape)
        if clock_valid and metrics is not None:
            require(len(medians) == 9 and math.isclose(sum(medians) / len(medians),
                metrics['balanced_phase_shape_median_ms'], abs_tol=1e-12), 'balanced query median')
    return problems, limitations


def audit(root, summary, cell):
    actual = off_checks(root, summary) if cell == 'off' else independent.checks(root, summary)
    extra, limits = supplemental(root, summary, cell)
    return actual + extra, limits


def adapter(source, scratch):
    destination = scratch / source.name
    destination.mkdir()
    for p in source.iterdir():
        if p.is_file() and p.name not in ('resources.json', 'resources.json.gz'):
            (destination / p.name).symlink_to(p.resolve())
    path = source / 'resources.json.gz'
    opener = gzip.open if path.exists() else open
    if not path.exists():
        path = source / 'resources.json'
    with opener(path, 'rb') as src, (destination / 'resources.json').open('wb') as dst:
        shutil.copyfileobj(src, dst)
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--data', type=Path, default=DATA)
    parser.add_argument('--out', type=Path, default=DATA / 'audit-reviewed.json')
    parser.add_argument('--cells', nargs='+', choices=('off', 'scan', 'walk'), default=('off', 'scan'))
    args = parser.parse_args()
    require_limits()
    scratch_parent = Path(os.environ['TMPDIR']).resolve()
    if not scratch_parent.is_relative_to(STORAGE.resolve() / 'scratch'):
        raise RuntimeError('audit scratch must be launcher-owned data drive TMPDIR')
    if args.out.exists() or not args.out.resolve().is_relative_to(DATA.resolve()):
        raise RuntimeError('audit output must be fresh and in owned evidence directory')
    checker = Path(independent.__file__)
    report = {'independent_checker': str(checker.relative_to(ROOT)),
        'independent_checker_sha256': hashlib.sha256(checker.read_bytes()).hexdigest(),
        'audit_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        'origin': 'Scan/Walk unchanged checker checks(); Off scoped adaptation; supplemental independently derives phase metrics',
        'cells': {}, 'passed': False}
    fixture = independent.read(DATA / 'clock-rounding-counterexample.json')
    fixture_offsets = [fixture['min_offset_ns'], fixture['max_offset_ns']]
    rounded = clock_range(fixture_offsets, fixture['declared_range_ms'])
    changed_clock = clock_range(fixture_offsets, fixture['declared_range_ms'] + 1)
    report['rounding_counterexample'] = {'comparison': rounded,
        'one_ms_mutation': changed_clock,
        'passed': rounded['accounting_consistent'] and not changed_clock['accounting_consistent']}
    with tempfile.TemporaryDirectory(prefix='query-accounting-audit-', dir=scratch_parent) as temporary:
        owned = Path(temporary)
        report['scratch'] = str(owned)
        for cell in args.cells:
            root = adapter(args.data / cell, owned)
            summary = independent.read(root / 'summary.json')
            actual, limitations = audit(root, summary, cell)
            controls = {}
            expected_rejections = {'rate': 'burst rate spool_batches', 'cpu': 'server CPU mean',
                'phase_cpu': 'normal CPU mean', 'custody': 'source/cycle/recovery log totals differ',
                'rss': 'offered phase RSS', 'clock_range': 'clock-offset range raw recomputation',
                'query_p99': 'query p99 recent_logs', 'query_median': 'normal query median recent_logs'}
            additions = {}
            control_passed = {}
            for defect in ('rate', 'cpu', 'phase_cpu', 'custody', 'clock_range', *(() if cell == 'scan' else ('rss',)),
                           *(() if cell == 'off' else ('query_p99', 'query_median'))):
                changed = copy.deepcopy(summary)
                if defect == 'rate':
                    changed['observation']['rates']['burst']['per_second']['spool_batches'] += 1
                elif defect == 'cpu':
                    changed['observation']['cpu']['whole']['processes']['server']['mean_cores'] += 1
                elif defect == 'phase_cpu':
                    changed['observation']['cpu']['normal']['processes']['server']['mean_cores'] += 1
                elif defect == 'rss':
                    changed['lab']['metrics']['offered_phase_peak_server_rss_mib'] += 1
                elif defect == 'custody':
                    changed['recovered_logs'] -= 1
                elif defect == 'clock_range':
                    changed['clock_offset_range_ms'] += 1
                elif defect == 'query_p99':
                    changed['observation']['queries']['recent_logs']['all']['latency_ms']['p99'] += 1
                else:
                    changed['observation']['queries']['recent_logs']['normal']['latency_ms']['p50'] += 1
                controls[defect] = audit(root, changed, cell)[0]
                additions[defect] = sorted(set(controls[defect]) - set(actual))
                control_passed[defect] = expected_rejections[defect] in additions[defect]
            resources = independent.read(root / 'resources.json')
            clock = clock_range([r['wall_ns'] - r['mono_ns'] for r in resources], summary['clock_offset_range_ms'])
            report['cells'][cell] = {'problems': actual, 'negative_controls': controls,
                'negative_control_additions': additions, 'negative_control_passed': control_passed,
                'clock_range': clock,
                'skips': ['query latency population', 'visibility sentinel census/joins'] if cell == 'off' else [],
                'limitations': limitations, 'original_run_passed': summary['passed'],
                'performance_eligible': summary['passed'] and summary['gates']['clock_offset_range_le_5ms'] and clock['integer_range_le_5ms'],
                'passed': not actual and all(control_passed.values())}
    report['scratch_removed'] = not Path(report['scratch']).exists()
    report['passed'] = report['scratch_removed'] and report['rounding_counterexample']['passed'] and all(c['passed'] for c in report['cells'].values())
    args.out.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))
    raise SystemExit(0 if report['passed'] else 1)


if __name__ == '__main__':
    main()
