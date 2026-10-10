"""Grade fixed range-evidence records; no execution or oracle rerun occurs here."""
import copy
import hashlib
import json
import math
import re
import statistics

CELLS = {
    'A': {'n': 16, 'body_bytes': 128, 'included': 8, 'nodes': 1},
    'B': {'n': 16, 'body_bytes': 4096, 'included': 8, 'nodes': 1},
    'C': {'n': 64, 'body_bytes': 4096, 'included': 32, 'nodes': 1},
    'D': {'n': 64, 'body_bytes': 4096, 'included': 56, 'nodes': 8},
}
PHASES = ('decode_fold', 'end_scan')
ARMS = ('rows', 'metadata')
REPEATS = 16
# Exact producer control names; these records are checked, not rerun here.
CONTROL_NAMES = (
    'malformed-envelope', 'malformed-node', 'malformed-logs',
    'malformed-metrics', 'malformed-traces', 'empty-signals', 'gap-only',
    'zero-observation', 'unsupported-histogram', 'excluded-malformed-otlp',
    'zero-inclusion', 'middle-interval', 'missing-projections',
    'corrupt-projections', 'excluded-raw-digest', 'decode-error-order',
)


def _integer(value, name, minimum=0, maximum=(1 << 64) - 1):
    if type(value) is not int or not minimum <= value <= maximum:
        raise ValueError('invalid integer ' + name)
    return value


def _expected(cell):
    lo, hi = (9, 64) if cell == 'D' else (1, CELLS[cell]['included'])
    freshness = {}
    for index in range(lo - 1, hi):
        node = 'node-' + str(index % CELLS[cell]['nodes'])
        freshness[node] = 2000 + 100 * index + 31
    return {'received': [1000 + 100 * (lo - 1), 1000 + 100 * (hi - 1)],
            'freshness': freshness}


def _same_json(left, right):
    return json.dumps(left, sort_keys=True, separators=(',', ':'), allow_nan=False) == json.dumps(
        right, sort_keys=True, separators=(',', ':'), allow_nan=False)


def _parse(events, counted):
    if type(events) is not list or not events:
        raise ValueError('nonempty parsed event list required')
    if any(type(record) is not dict for record in events):
        raise ValueError('every event must be an object')
    if events[0].get('event') != 'config' or events[-1].get('event') != 'complete':
        raise ValueError('config/complete framing required')
    config = events[0]
    if (type(config.get('seed')) is not int or config['seed'] != 42
            or type(config.get('repeats')) is not int or config['repeats'] != REPEATS
            or config.get('cells') != list(CELLS)
            or config.get('allocator_counted') is not counted):
        raise ValueError('config differs from fixed protocol')
    cells, trials = {}, {}
    complete = events[-1]
    for record in events[1:-1]:
        event = record.get('event')
        cell = record.get('cell')
        if cell not in CELLS:
            raise ValueError('unknown cell')
        if event == 'cell':
            if cell in cells:
                raise ValueError('duplicate cell')
            for name, value in CELLS[cell].items():
                if type(record.get(name)) is not int or record[name] != value:
                    raise ValueError('fixture association mismatch: ' + cell + '/' + name)
            expected_range = [9, 64] if cell == 'D' else [1, CELLS[cell]['included']]
            if not _same_json(record.get('range'), expected_range):
                raise ValueError('fixture snapshot range mismatch')
            digest = record.get('fixture_sha256')
            if type(digest) is not str or not re.fullmatch('[0-9a-f]{64}', digest):
                raise ValueError('invalid fixture digest')
            _integer(record.get('fixture_encoded_bytes'), 'fixture_encoded_bytes', 1)
            raw_bytes = _integer(record.get('raw_batch_bytes'), 'raw_batch_bytes', 1)
            included_bytes = _integer(record.get('included_batch_bytes'), 'included_batch_bytes', 1)
            if included_bytes > raw_bytes:
                raise ValueError('included bytes exceed raw fixture bytes')
            if (_integer(record.get('included_observations'), 'included_observations')
                    != CELLS[cell]['included'] * 10):
                raise ValueError('fixture observation count mismatch')
            if not _same_json(record.get('expected'), _expected(cell)):
                raise ValueError('declared expectation differs from independent arithmetic')
            controls = record.get('controls')
            if (type(controls) is not list or len(controls) != len(CONTROL_NAMES)
                    or any(type(row) is not dict or type(row.get('name')) is not str for row in controls)
                    or {row.get('name') for row in controls} != set(CONTROL_NAMES)
                    or any(row.get('passed') is not True for row in controls)):
                raise ValueError('missing/duplicate/failed recorded controls')
            cells[cell] = record
        elif event == 'trial':
            phase, arm, pair = record.get('phase'), record.get('arm'), record.get('pair')
            if phase not in PHASES or arm not in ARMS or type(pair) is not int or pair not in (1, 2, 3):
                raise ValueError('invalid trial identity')
            key = cell, phase, pair, arm
            if key in trials:
                raise ValueError('duplicate trial')
            expected_position = (1 if arm == 'rows' else 2) if pair != 2 else (2 if arm == 'rows' else 1)
            if (type(record.get('position')) is not int or record['position'] != expected_position
                    or type(record.get('repeats')) is not int or record['repeats'] != REPEATS
                    or record.get('allocator_counted') is not counted):
                raise ValueError('trial order/repeats/observer mismatch')
            _integer(record.get('wall_ns'), 'wall_ns', 1, (1 << 128) - 1)
            _integer(record.get('cpu_ns'), 'cpu_ns', 0, (1 << 128) - 1)
            for name in ('requested_bytes', 'allocation_calls', 'peak_extra_requested_bytes'):
                _integer(record.get(name), name)
            if not counted and any(record[name] != 0 for name in ('requested_bytes', 'allocation_calls', 'peak_extra_requested_bytes')):
                raise ValueError('plain trial contains counted allocations')
            if counted and record['requested_bytes'] == 0:
                raise ValueError('counted allocation denominator is zero')
            if counted and record['allocation_calls'] == 0:
                raise ValueError('counted allocation calls are zero')
            if record.get('expected_match') is not True or not _same_json(record.get('actual'), _expected(cell)):
                raise ValueError('trial differs from independent expectation')
            trials[key] = record
        else:
            raise ValueError('unexpected event')
    expected_keys = {(cell, phase, pair, arm) for cell in CELLS for phase in PHASES
                     for pair in (1, 2, 3) for arm in ARMS}
    if set(cells) != set(CELLS) or set(trials) != expected_keys:
        raise ValueError('incomplete cell/trial matrix')
    # Producer emits trials before the cell record; associate after parsing all.
    for (cell, phase, _pair, _arm), record in trials.items():
        work = {
            'raw_bytes_hashed_logical': 0 if phase == 'decode_fold' else cells[cell]['raw_batch_bytes'] * REPEATS,
            'included_entries': CELLS[cell]['included'] * REPEATS,
            'included_observations': CELLS[cell]['included'] * 10 * REPEATS,
        }
        for name, expected in work.items():
            if _integer(record.get(name), name) != expected:
                raise ValueError('derived work association mismatch: ' + name)
    if (type(complete.get('trials')) is not int or complete['trials'] != len(expected_keys)
            or type(complete.get('controls')) is not int
            or complete['controls'] != len(CELLS) * len(CONTROL_NAMES)
            or complete.get('controls_all_passed') is not True):
        raise ValueError('complete count/control mismatch')
    _integer(complete.get('fixture_file_bytes'), 'fixture_file_bytes', 1, 2 * 1024 * 1024)
    # This binds parsed records, not the original command stdout byte stream.
    canonical = json.dumps(events, sort_keys=True, separators=(',', ':'), allow_nan=False).encode()
    return cells, trials, hashlib.sha256(canonical).hexdigest()


def _ratios(trials, cell, phase, metric):
    ratios = []
    for pair in (1, 2, 3):
        baseline = trials[cell, phase, pair, 'rows'][metric]
        candidate = trials[cell, phase, pair, 'metadata'][metric]
        if baseline <= 0:
            return None
        ratio = candidate / baseline
        if not math.isfinite(ratio):
            raise ValueError('nonfinite derived ratio')
        ratios.append(ratio)
    return ratios


def summarize(plain_events, counted_events):
    """Validate two parsed JSONL passes and return the finite-screen decision."""
    plain_cells, plain, plain_sha = _parse(plain_events, False)
    counted_cells, counted, counted_sha = _parse(counted_events, True)
    for cell in CELLS:
        for name in ('fixture_sha256', 'fixture_encoded_bytes', 'expected',
                     'raw_batch_bytes', 'included_batch_bytes', 'included_observations'):
            if plain_cells[cell][name] != counted_cells[cell][name]:
                raise ValueError('plain/count fixture provenance mismatch')
    result = {}
    timing_guards, allocation_guards, improved = [], [], []
    for cell in CELLS:
        phases = {}
        for phase in PHASES:
            ratios = _ratios(plain, cell, phase, 'wall_ns')
            median = statistics.median(ratios)
            guard = median <= 1.05
            timing_guards.append(guard)
            allocation = _ratios(counted, cell, phase, 'requested_bytes')
            allocation_median = statistics.median(allocation)
            calls = _ratios(counted, cell, phase, 'allocation_calls')
            peak = _ratios(counted, cell, phase, 'peak_extra_requested_bytes')
            phases[phase] = {
                'logical_work_per_arm': {
                    'raw_bytes_hashed_logical': plain[cell, phase, 1, 'rows']['raw_bytes_hashed_logical'],
                    'included_entries': CELLS[cell]['included'] * REPEATS,
                    'included_observations': CELLS[cell]['included'] * 10 * REPEATS,
                    'scope': 'fixture-derived counts; not measured syscalls or device I/O',
                },
                'plain_paired_wall_ratios': ratios,
                'plain_median_wall_ratio': median,
                'plain_median_improvement_pct': 100 * (1 - median),
                'plain_median_speedup_factor': 1 / median,
                'plain_timing_guard': guard,
                'plain_rows_median_ns_per_repeat': statistics.median(
                    plain[cell, phase, pair, 'rows']['wall_ns'] / REPEATS for pair in (1, 2, 3)),
                'plain_metadata_median_ns_per_repeat': statistics.median(
                    plain[cell, phase, pair, 'metadata']['wall_ns'] / REPEATS for pair in (1, 2, 3)),
                'plain_process_cpu_ratios': _ratios(plain, cell, phase, 'cpu_ns'),
                'cpu_scope': 'process CPU diagnostic; null ratios mean a zero baseline',
                'counted_requested_byte_ratios': allocation,
                'counted_median_allocation_reduction_pct': 100 * (1 - allocation_median),
                'counted_allocation_call_ratios': calls,
                'counted_peak_extra_requested_ratios': peak,
            }
            allocation_guards.append(allocation_median < 1)
            if phase == 'end_scan':
                improved.append(median <= 0.9)
        result[cell] = phases
    return {
        'cells': result,
        'fixture_work': {cell: {name: plain_cells[cell][name] for name in
                         ('raw_batch_bytes', 'included_batch_bytes', 'included_observations')}
                         for cell in CELLS},
        'nomination': sum(improved) >= 2 and all(timing_guards) and all(allocation_guards),
        'end_scan_cells_with_at_least_10pct_median_improvement': sum(improved),
        'all_plain_phase_median_regression_guards': all(timing_guards),
        'all_phase_requested_allocation_reductions': all(allocation_guards),
        'known_expectation_checks': 96,
        'paired_arm_equivalence_checks': 48,
        'recorded_controls_per_pass': len(CELLS) * len(CONTROL_NAMES),
        'input_parsed_event_sha256': {'plain': plain_sha, 'counted': counted_sha},
        'scope': 'finite warm-cache microbenchmark; counted timing is diagnostic; no query-QPS claim',
        'audit_scope': 'parsed record grading; no control/oracle rerun or original stdout/archive verification',
    }


def negative_controls(plain_events, counted_events):
    """Inject isolated defects; return names rejected by structure or decision.

    These test the grader, not the native oracle. Valid positive timings cannot
    establish tampering: original stdout and binary hashes belong to the driver.
    """
    summarize(plain_events, counted_events)
    rejected = []

    def check(name, mutate, failed_guard=None):
        plain, counted = copy.deepcopy(plain_events), copy.deepcopy(counted_events)
        mutate(plain, counted)
        try:
            result = summarize(plain, counted)
        except ValueError:
            if failed_guard is not None:
                raise ValueError('decision control corrupted evidence: ' + name)
        else:
            if (failed_guard is None or result['nomination']
                    or result[failed_guard] is not False):
                raise ValueError('negative control accepted: ' + name)
        rejected.append(name)

    def first(events, event):
        return next(record for record in events if record.get('event') == event)

    def missing(plain, _counted):
        plain.remove(first(plain, 'trial'))

    def duplicate(plain, _counted):
        plain.insert(-1, copy.deepcopy(first(plain, 'trial')))

    def output(plain, _counted):
        actual = first(plain, 'trial')['actual']['freshness']
        node = next(iter(actual))
        actual[node] += 1

    def provenance(_plain, counted):
        record = first(counted, 'cell')
        record['fixture_sha256'] = '0' * 64 if record['fixture_sha256'] != '0' * 64 else '1' * 64

    def order(plain, _counted):
        record = first(plain, 'trial')
        record['position'] = 3 - record['position']

    def nonfinite(plain, _counted):
        first(plain, 'trial')['wall_ns'] = float('nan')

    def zero_allocation(_plain, counted):
        first(counted, 'trial')['requested_bytes'] = 0

    def failed_control(plain, _counted):
        first(plain, 'cell')['controls'][0]['passed'] = False

    def footer(plain, _counted):
        plain[-1]['trials'] -= 1

    def work_association(plain, _counted):
        first(plain, 'trial')['included_observations'] += 1

    def timing_guard(plain, _counted):
        rows = {(r['cell'], r['phase'], r['pair']): r for r in plain
                if r.get('event') == 'trial' and r['arm'] == 'rows'}
        for record in plain:
            if record.get('event') == 'trial' and record['arm'] == 'metadata':
                baseline = rows[record['cell'], record['phase'], record['pair']]['wall_ns']
                record['wall_ns'] = (baseline * 5 + 3) // 4

    def allocation_guard(_plain, counted):
        rows = {(r['cell'], r['phase'], r['pair']): r for r in counted
                if r.get('event') == 'trial' and r['arm'] == 'rows'}
        # Fail only decode-fold to verify both phases enter the decision.
        for record in counted:
            if (record.get('event') == 'trial' and record['arm'] == 'metadata'
                    and record['phase'] == 'decode_fold'):
                record['requested_bytes'] = rows[record['cell'], record['phase'], record['pair']]['requested_bytes']

    for name, mutate in (
            ('missing_trial', missing), ('duplicate_trial', duplicate),
            ('changed_known_output', output), ('fixture_provenance_mismatch', provenance),
            ('wrong_pair_order', order), ('nonfinite_wall', nonfinite),
            ('zero_counted_allocation', zero_allocation),
            ('failed_recorded_control', failed_control), ('changed_complete_count', footer),
            ('changed_work_association', work_association)):
        check(name, mutate)
    check('timing_regression_guard', timing_guard,
          failed_guard='all_plain_phase_median_regression_guards')
    check('decode_fold_allocation_guard', allocation_guard,
          failed_guard='all_phase_requested_allocation_reductions')
    return rejected
