"""Independent timing/backlog grading for the registered local main cells."""
import json
import math
from pathlib import Path

NS = 1_000_000_000


def p99(values):
    ordered = sorted(values)
    return ordered[max(0, math.ceil(.99 * len(ordered)) - 1)] if ordered else None


def population(entries, begin_ns, end_ns):
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
    return {'passed': valid and complete and peak is not None and peak <= 1
            and all(v['last'] <= v['first'] for v in means.values())
            and all(e.get('ack_ns') is not None for e in entries),
            'valid': valid, 'complete_measured_population': complete,
            'measured_batches': len(measured), 'censored': sum(e.get('ack_ns') is None for e in measured),
            'creation_to_ack_p99_s': peak, 'backlog_means': means, 'samples': samples,
            'final_unacked': sum(e.get('ack_ns') is None for e in entries)}


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
