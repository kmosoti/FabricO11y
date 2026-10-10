"""Independent timing/backlog grading for the registered local main cells."""
import json
import math
from pathlib import Path

NS = 1_000_000_000


def p99(values):
    ordered = sorted(values)
    return ordered[max(0, math.ceil(.99 * len(ordered)) - 1)] if ordered else None


def population(entries, begin_ns, end_ns, *, rule='sampled-v1'):
    if rule not in ('sampled-v1', 'clearing-v2'):
        raise ValueError('unregistered backlog decision')
    if end_ns <= begin_ns:
        raise ValueError('nonpositive measurement window')
    measured = [e for e in entries if begin_ns <= e['created_ns'] < end_ns]
    valid = all(type(e.get('created_ns')) is int and type(e.get('bytes')) is int
                and e['bytes'] > 0 and (e.get('ack_ns') is None
                or type(e['ack_ns']) is int and e['ack_ns'] >= e['created_ns']) for e in entries)
    complete = bool(measured) and all(e.get('ack_ns') is not None for e in measured)
    latencies = [(e['ack_ns'] - e['created_ns']) / NS for e in measured if e.get('ack_ns') is not None]
    samples = []
    for at in range(begin_ns, end_ns, 5 * NS):
        pending = [e for e in entries if e['created_ns'] <= at and (e.get('ack_ns') is None or e['ack_ns'] > at)]
        samples.append({'at_ns': at, 'count': len(pending), 'bytes': sum(e['bytes'] for e in pending)})
    first = [s for s in samples if s['at_ns'] < begin_ns + 30 * NS]
    last = [s for s in samples if s['at_ns'] >= end_ns - 30 * NS]
    means = {name: {'first': sum(s[name] for s in first) / len(first),
                    'last': sum(s[name] for s in last) / len(last)} for name in ('count', 'bytes')}
    peak = p99(latencies)
    result = {'passed': valid and complete and peak is not None and peak <= 1
            and all(v['last'] <= v['first'] for v in means.values())
            and all(e.get('ack_ns') is not None for e in entries),
            'valid': valid, 'complete_measured_population': complete,
            'measured_batches': len(measured), 'censored': sum(e.get('ack_ns') is None for e in measured),
            'creation_to_ack_p99_s': peak, 'backlog_means': means, 'samples': samples,
            'final_unacked': sum(e.get('ack_ns') is None for e in entries)}
    if rule == 'clearing-v2':
        result['sampled_v1_passed'] = result['passed']
        result['clearing'] = clearing(entries, begin_ns, end_ns)
        result['passed'] = (valid and complete and peak is not None and peak <= 1
                            and result['final_unacked'] == 0 and result['clearing']['passed'])
    result['backlog_rule'] = rule
    return result


def clearing(entries, begin_ns, end_ns):
    """Full interval union: accepted work must repeatedly drain within 5 s.

    This deliberately makes no stationarity or nonincreasing-occupancy claim.
    Adjacent intervals are merged before selecting periods crossing the window.
    """
    if end_ns <= begin_ns:
        raise ValueError('nonpositive measurement window')
    valid = bool(entries) and all(type(e.get('created_ns')) is int and e['created_ns'] >= 0
                and type(e.get('ack_ns')) is int and e['ack_ns'] >= e['created_ns']
                and type(e.get('bytes')) is int and e['bytes'] > 0 for e in entries)
    if not valid:
        return {'passed': False, 'complete_valid_population': False}
    events = {}
    for entry in entries:
        if entry['ack_ns'] == entry['created_ns']:
            continue
        for stamp, direction in [(entry['created_ns'], 1), (entry['ack_ns'], -1)]:
            delta = events.setdefault(stamp, [0, 0])
            delta[0] += direction
            delta[1] += direction * entry['bytes']
    periods, count, size, opened = [], 0, 0, None
    max_count = max_bytes = 0
    ordered = sorted(events)
    for index, stamp in enumerate(ordered):
        before = count
        count += events[stamp][0]
        size += events[stamp][1]
        if before == 0 and count > 0:
            opened = stamp
        elif before > 0 and count == 0:
            periods.append((opened, stamp))
            opened = None
        # State applies on the interval starting at this timestamp. Including
        # the next event handles a queue already nonempty at the window start.
        next_stamp = ordered[index + 1] if index + 1 < len(ordered) else stamp
        if stamp < end_ns and next_stamp > begin_ns:
            max_count, max_bytes = max(max_count, count), max(max_bytes, size)
    relevant = [(left, right) for left, right in periods if left < end_ns and right > begin_ns]
    longest = max((right-left for left, right in relevant), default=0)
    windows = []
    for left in range(begin_ns, end_ns, 5 * NS):
        right = min(left + 5 * NS, end_ns)
        occupied = sum(max(0, min(right, b)-max(left, a)) for a,b in relevant)
        windows.append({'from_ns': left, 'to_ns': right, 'empty_ns': right-left-occupied})
    means = []
    for left, right in [(begin_ns, min(begin_ns+30*NS, end_ns)),
                        (max(begin_ns, end_ns-30*NS), end_ns)]:
        parts = [(max(0, min(right, e['ack_ns'])-max(left, e['created_ns'])), e['bytes'])
                 for e in entries]
        means.append({'count': sum(d for d,_ in parts)/(right-left),
                      'bytes': sum(d*b for d,b in parts)/(right-left)})
    return {'passed': longest <= 5 * NS and all(w['empty_ns'] > 0 for w in windows),
            'complete_valid_population': True, 'max_busy_ns': longest,
            'busy_periods_intersecting_window': len(relevant),
            'queue_empty_fraction': sum(w['empty_ns'] for w in windows)/(end_ns-begin_ns),
            'max_outstanding_count': max_count, 'max_outstanding_bytes': max_bytes,
            'continuous_first_last_mean': means, 'windows': windows}


def native(log, raw_sources):
    """Join each real CLI delivery to same-sequence monotonic timing in order."""
    by_sequence = {r['sequence']: r for r in raw_sources}
    if len(by_sequence) != len(raw_sources) or not raw_sources:
        raise ValueError('one complete native source stream required')
    stream = (raw_sources[0]['node_id'], raw_sources[0]['generation'])
    events, delivery = {}, {}
    for line in Path(log).read_text().splitlines():
        if not line.startswith(('timing ', 'delivery ')):
            continue
        values = dict(part.split('=', 1) for part in line.split()[1:])
        if 'dropped_events' in values:
            raise ValueError('native timing events were dropped')
        sequence = int(values['sequence'])
        if sequence not in by_sequence:
            raise ValueError('timing/delivery has no independently retained source')
        if line.startswith('delivery '):
            delivery.setdefault(sequence, []).append(values)
        else:
            if (values['node_id'], int(values['generation'])) != stream:
                raise ValueError('timing stream identity changed')
            if values['boot_monotonic_ns'] == 'unmeasured':
                raise ValueError('missing same-boot timing stamp')
            events.setdefault(sequence, {}).setdefault(values['stage'], []).append(int(values['boot_monotonic_ns']))
    result = []
    import base64
    import hashlib
    for sequence, source in by_sequence.items():
        raw = base64.b64decode(source['bytes'], validate=True)
        stamps = events.get(sequence, {})
        accepted = stamps.get('sources_accepted', stamps.get('traces_accepted', []))
        committed = stamps.get('spool_committed', [])
        starts, answers = stamps.get('send_started', []), stamps.get('answer_received', [])
        attempts = delivery.get(sequence, [])
        if len(accepted) != 1 or len(committed) != 1 or len(starts) != len(answers) or len(answers) != len(attempts):
            raise ValueError('native source/commit/attempt timing population incomplete')
        if committed[0] < accepted[0] or any(a < s for s, a in zip(starts, answers)):
            raise ValueError('native monotonic stage order violated')
        ack = None
        for attempt, answer in zip(attempts, answers):
            if attempt['sha256'] != hashlib.sha256(raw).hexdigest():
                raise ValueError('native attempt differs from retained source bytes')
            if attempt['status'] == 'ack':
                if int(attempt['committed_through']) != sequence:
                    raise ValueError('unexpected native ACK sequence')
                if ack is None:
                    ack = answer
        result.append({'key': [*stream, sequence], 'created_ns': accepted[0],
                       'ack_ns': ack, 'bytes': len(raw), 'spool_s': (committed[0] - accepted[0]) / NS,
                       'attempts': len(attempts), 'retries': sum(a['status'] != 'ack' for a in attempts),
                       'first_send_to_ack_s': None if ack is None or not starts else (ack - starts[0]) / NS})
    return result
