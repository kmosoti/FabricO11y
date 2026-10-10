"""Grade exact raw-entry copy elimination; timing is a nonregression guard."""
import copy
import hashlib
import json
import math
import re
import statistics

from range_evidence_summary import CELLS, CONTROL_NAMES, _expected, _integer, _same_json

ARMS = ('owned', 'borrowed')
PAIRS = (1, 2, 3, 4, 5)
REPEATS = 64
CONTROLS = CONTROL_NAMES + ('borrowed-lifetime-unicode',)


def _parse(events, counted):
    if (type(events) is not list or len(events) < 2
            or any(type(row) is not dict for row in events)
            or events[0].get('event') != 'config' or events[-1].get('event') != 'complete'):
        raise ValueError('invalid config/complete framing')
    config = events[0]
    if (type(config.get('seed')) is not int or config['seed'] != 42
            or type(config.get('repeats')) is not int or config['repeats'] != REPEATS
            or config.get('cells') != list(CELLS)
            or config.get('allocator_counted') is not counted):
        raise ValueError('config differs from fixed borrowed-copy protocol')
    cells, trials = {}, {}
    for row in events[1:-1]:
        cell = row.get('cell')
        if type(cell) is not str or cell not in CELLS:
            raise ValueError('unknown cell')
        if row.get('event') == 'cell':
            if cell in cells:
                raise ValueError('duplicate cell')
            for name, value in CELLS[cell].items():
                if type(row.get(name)) is not int or row[name] != value:
                    raise ValueError('fixture association mismatch')
            bounds = [9, 64] if cell == 'D' else [1, CELLS[cell]['included']]
            if not _same_json(row.get('range'), bounds) or not _same_json(row.get('expected'), _expected(cell)):
                raise ValueError('independent fixture expectation mismatch')
            if type(row.get('fixture_sha256')) is not str or not re.fullmatch('[0-9a-f]{64}', row['fixture_sha256']):
                raise ValueError('invalid fixture SHA')
            for name in ('fixture_encoded_bytes', 'raw_batch_bytes', 'included_batch_bytes',
                         'all_label_bytes', 'distinct_included_label_bytes', 'distinct_included_labels'):
                _integer(row.get(name), name, 1)
            if (row['included_batch_bytes'] > row['raw_batch_bytes']
                    or row['distinct_included_labels'] != CELLS[cell]['nodes']
                    or row['distinct_included_label_bytes'] > row['all_label_bytes']):
                raise ValueError('invalid copy prediction inputs')
            # Main timed fixture labels are node-i; the unequal Unicode fixture
            # is an additional semantic control, not an allocation denominator.
            all_labels = ['node-' + str(index % CELLS[cell]['nodes'])
                          for index in range(CELLS[cell]['n'])]
            lo, hi = bounds
            included_labels = set(all_labels[lo - 1:hi])
            if (row['all_label_bytes'] != sum(len(label.encode('utf-8')) for label in all_labels)
                    or row['distinct_included_label_bytes'] != sum(len(label.encode('utf-8')) for label in included_labels)):
                raise ValueError('label byte prediction differs from fixture arithmetic')
            if _integer(row.get('included_observations'), 'included_observations') != CELLS[cell]['included'] * 10:
                raise ValueError('included observations mismatch')
            controls = row.get('controls')
            if (type(controls) is not list or len(controls) != len(CONTROLS)
                    or any(type(control) is not dict or type(control.get('name')) is not str for control in controls)
                    or {control['name'] for control in controls} != set(CONTROLS)
                    or any(control.get('passed') is not True for control in controls)):
                raise ValueError('missing/duplicate/failed recorded controls')
            cells[cell] = row
        elif row.get('event') == 'trial':
            pair, arm = row.get('pair'), row.get('arm')
            if (type(pair) is not int or pair not in PAIRS
                    or arm not in ARMS or row.get('phase') != 'end_scan'):
                raise ValueError('invalid trial identity')
            key = cell, pair, arm
            if key in trials:
                raise ValueError('duplicate trial')
            position = (1 if arm == 'owned' else 2) if pair % 2 else (2 if arm == 'owned' else 1)
            if (type(row.get('position')) is not int or row['position'] != position
                    or type(row.get('repeats')) is not int or row['repeats'] != REPEATS
                    or row.get('allocator_counted') is not counted):
                raise ValueError('trial order/repeats/observer mismatch')
            for name in ('wall_ns', 'cpu_ns'):
                _integer(row.get(name), name, 1, (1 << 128) - 1)
            for name in ('requested_bytes', 'allocation_calls', 'peak_extra_requested_bytes'):
                _integer(row.get(name), name)
            if not counted and any(row[name] for name in ('requested_bytes', 'allocation_calls', 'peak_extra_requested_bytes')):
                raise ValueError('plain pass contains counted allocations')
            if counted and (not row['requested_bytes'] or not row['allocation_calls']):
                raise ValueError('counted allocation denominator zero')
            if row.get('expected_match') is not True or not _same_json(row.get('actual'), _expected(cell)):
                raise ValueError('trial known expectation mismatch')
            trials[key] = row
        else:
            raise ValueError('unexpected event')
    keys = {(cell, pair, arm) for cell in CELLS for pair in PAIRS for arm in ARMS}
    if set(cells) != set(CELLS) or set(trials) != keys:
        raise ValueError('incomplete trial/cell matrix')
    for (cell, _pair, _arm), row in trials.items():
        for name, expected in {
                'raw_bytes_hashed_logical': cells[cell]['raw_batch_bytes'] * REPEATS,
                'included_entries': CELLS[cell]['included'] * REPEATS,
                'included_observations': CELLS[cell]['included'] * 10 * REPEATS,
                'predicted_saved_bytes': (cells[cell]['raw_batch_bytes'] + cells[cell]['all_label_bytes']
                                          - cells[cell]['distinct_included_label_bytes']) * REPEATS,
                'predicted_saved_calls': (2 * CELLS[cell]['n'] - cells[cell]['distinct_included_labels']) * REPEATS}.items():
            if _integer(row.get(name), name) != expected:
                raise ValueError('logical work association mismatch')
    complete = events[-1]
    if (type(complete.get('trials')) is not int or complete['trials'] != len(keys)
            or type(complete.get('controls')) is not int or complete['controls'] != len(CELLS) * len(CONTROLS)
            or complete.get('controls_all_passed') is not True):
        raise ValueError('completion count mismatch')
    _integer(complete.get('fixture_file_bytes'), 'fixture_file_bytes', 1, 2 * 1024 * 1024)
    digest = hashlib.sha256(json.dumps(events, sort_keys=True, separators=(',', ':'), allow_nan=False).encode()).hexdigest()
    return cells, trials, digest


def summarize(plain_events, counted_events):
    """Return mechanism admission; no wall-speed promotion criterion exists."""
    plain_cells, plain, plain_sha = _parse(plain_events, False)
    counted_cells, counted, counted_sha = _parse(counted_events, True)
    results, guards = {}, []
    fixture_fields = ('fixture_sha256', 'fixture_encoded_bytes', 'raw_batch_bytes',
                      'included_batch_bytes', 'all_label_bytes', 'distinct_included_label_bytes',
                      'distinct_included_labels', 'included_observations', 'expected')
    for cell in CELLS:
        fixture = plain_cells[cell]
        if any(not _same_json(fixture[name], counted_cells[cell][name]) for name in fixture_fields):
            raise ValueError('plain/counted fixture provenance mismatch')
        predicted_bytes = (fixture['raw_batch_bytes'] + fixture['all_label_bytes']
                           - fixture['distinct_included_label_bytes']) * REPEATS
        predicted_calls = (2 * CELLS[cell]['n'] - fixture['distinct_included_labels']) * REPEATS
        ratios = {}
        for metric in ('wall_ns', 'cpu_ns'):
            values = [plain[cell, pair, 'borrowed'][metric] / plain[cell, pair, 'owned'][metric]
                      for pair in PAIRS]
            if not all(math.isfinite(value) for value in values):
                raise ValueError('nonfinite paired ratio')
            ratios[metric] = values
        byte_deltas, call_deltas, requested_ratios, peak_deltas = [], [], [], []
        for pair in PAIRS:
            owned, borrowed = counted[cell, pair, 'owned'], counted[cell, pair, 'borrowed']
            byte_deltas.append(owned['requested_bytes'] - borrowed['requested_bytes'])
            call_deltas.append(owned['allocation_calls'] - borrowed['allocation_calls'])
            requested_ratios.append(borrowed['requested_bytes'] / owned['requested_bytes'])
            peak_deltas.append(borrowed['peak_extra_requested_bytes'] - owned['peak_extra_requested_bytes'])
        local = {
            'exact_requested_byte_prediction': all(delta == predicted_bytes for delta in byte_deltas),
            'exact_allocation_call_prediction': all(delta == predicted_calls for delta in call_deltas),
            'requested_bytes_decreased_every_pair': all(ratio < 1 for ratio in requested_ratios),
            'peak_did_not_increase_every_pair': all(delta <= 0 for delta in peak_deltas),
            'wall_median_nonregression': statistics.median(ratios['wall_ns']) <= 1.05,
            'cpu_median_nonregression': statistics.median(ratios['cpu_ns']) <= 1.05,
        }
        guards.extend(local.values())
        results[cell] = {
            'predicted_requested_byte_delta_per_arm': predicted_bytes,
            'actual_requested_byte_deltas': byte_deltas,
            'predicted_allocation_call_delta_per_arm': predicted_calls,
            'actual_allocation_call_deltas': call_deltas,
            'counted_requested_byte_ratios': requested_ratios,
            'counted_peak_extra_requested_deltas': peak_deltas,
            'plain_paired_wall_ratios': ratios['wall_ns'],
            'plain_paired_cpu_ratios': ratios['cpu_ns'],
            'plain_median_wall_ratio': statistics.median(ratios['wall_ns']),
            'plain_median_cpu_ratio': statistics.median(ratios['cpu_ns']),
            'guards': local,
        }
    return {'cells': results, 'nomination': all(guards), 'mechanism_admitted': all(guards),
            'known_expectation_checks': 80, 'paired_arm_equivalence_checks': 40,
            'recorded_controls_per_pass': len(CELLS) * len(CONTROLS),
            'input_parsed_event_sha256': {'plain': plain_sha, 'counted': counted_sha},
            'scope': 'finite warm-cache exact copy-elimination objective; timing descriptive and guarded, no speed nomination or query QPS claim',
            'audit_scope': 'parsed record grading; no semantic-control rerun or original stdout/archive verification'}


def negative_controls(plain_events, counted_events):
    """Reject malformed evidence and falsify each mechanism/nonregression guard."""
    summarize(plain_events, counted_events)
    rejected = []

    def first(events, event):
        return next(row for row in events if row.get('event') == event)

    def check(name, mutate, failed_guard=None):
        plain, counted = copy.deepcopy(plain_events), copy.deepcopy(counted_events)
        mutate(plain, counted)
        try:
            result = summarize(plain, counted)
        except ValueError:
            if failed_guard is not None:
                raise ValueError('guard control corrupted evidence: ' + name)
        else:
            if (failed_guard is None or result['nomination']
                    or any(cell['guards'][failed_guard] is not False for cell in result['cells'].values())):
                raise ValueError('negative control accepted: ' + name)
        rejected.append(name)

    def missing(plain, _counted):
        plain.remove(first(plain, 'trial'))

    def duplicate(plain, _counted):
        plain.insert(-1, copy.deepcopy(first(plain, 'trial')))

    def output(plain, _counted):
        values = first(plain, 'trial')['actual']['freshness']
        values[next(iter(values))] += 1

    def provenance(_plain, counted):
        row = first(counted, 'cell')
        row['fixture_sha256'] = '0' * 64 if row['fixture_sha256'] != '0' * 64 else '1' * 64

    def order(plain, _counted):
        row = first(plain, 'trial')
        row['position'] = 3 - row['position']

    def nonfinite(plain, _counted):
        first(plain, 'trial')['wall_ns'] = float('nan')

    def control(plain, _counted):
        first(plain, 'cell')['controls'][0]['passed'] = False

    def work(plain, _counted):
        first(plain, 'trial')['raw_bytes_hashed_logical'] += 1

    def metric(events, name, change):
        owned = {(row['cell'], row['pair']): row for row in events
                 if row.get('event') == 'trial' and row['arm'] == 'owned'}
        for row in events:
            if row.get('event') == 'trial' and row['arm'] == 'borrowed':
                row[name] = change(row[name], owned[row['cell'], row['pair']][name])

    for name, mutation in (
            ('missing_trial', missing), ('duplicate_trial', duplicate),
            ('changed_known_output', output), ('fixture_provenance_mismatch', provenance),
            ('wrong_pair_order', order), ('nonfinite_wall', nonfinite),
            ('failed_recorded_control', control), ('changed_work_association', work)):
        check(name, mutation)
    check('wall_regression_guard',
          lambda plain, _counted: metric(plain, 'wall_ns', lambda _value, baseline: baseline * 2),
          'wall_median_nonregression')
    check('cpu_regression_guard',
          lambda plain, _counted: metric(plain, 'cpu_ns', lambda _value, baseline: baseline * 2),
          'cpu_median_nonregression')
    check('requested_byte_prediction',
          lambda _plain, counted: metric(counted, 'requested_bytes', lambda value, _baseline: value + 1),
          'exact_requested_byte_prediction')
    check('allocation_call_prediction',
          lambda _plain, counted: metric(counted, 'allocation_calls', lambda value, _baseline: value + 1),
          'exact_allocation_call_prediction')
    check('requested_byte_reduction_guard',
          lambda _plain, counted: metric(counted, 'requested_bytes', lambda _value, baseline: baseline),
          'requested_bytes_decreased_every_pair')
    check('peak_nonincrease_guard',
          lambda _plain, counted: metric(counted, 'peak_extra_requested_bytes', lambda _value, baseline: baseline + 1),
          'peak_did_not_increase_every_pair')
    return rejected
