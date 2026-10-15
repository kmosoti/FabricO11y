"""Independent joins over raw native browser event/clock observations.

Host brackets bound clock conversion. Neither rounded browser milliseconds nor
successful owner commands establish UI activity. All event rows must survive.
"""
import math

NS = 1_000_000_000


def require(value, message):
    if not value:
        raise ValueError('UI continuity: ' + message)


def validate(samples, start_ns, end_ns):
    require(type(start_ns) is int and type(end_ns) is int and 0 <= start_ns < end_ns,
            'invalid native interval')
    require(isinstance(samples, list) and len(samples) >= 2, 'missing native snapshots')
    offset_lo, offset_hi, identity, sequence, events, previous = None, None, None, 0, [], None
    for sample in samples:
        lo, hi = sample['mono_before_ns'], sample['mono_after_ns']
        require(type(lo) is int and type(hi) is int and 0 <= lo <= hi,
                'invalid host clock bracket')
        if previous is not None:
            require(previous <= lo, 'reversed clock observations')
        previous = hi
        now = sample['nowMs']
        require(type(now) in (int, float) and math.isfinite(now) and now >= 0,
                'invalid browser clock')
        current = (sample['documentId'], sample['timeOriginMs'], sample['webdriver_session'])
        require(all(current) and (identity is None or current == identity), 'document or session changed')
        identity = current
        # Permit one ms quantization, conservatively, on both boundaries.
        left, right = lo - math.ceil(now * 1_000_000) - 1_000_000, hi - math.floor(now * 1_000_000) + 1_000_000
        offset_lo = left if offset_lo is None else max(offset_lo, left)
        offset_hi = right if offset_hi is None else min(offset_hi, right)
        require(offset_lo <= offset_hi, 'inconsistent native/browser clocks')
        require(sample['visibility'] == 'visible' and not sample['eventOverflow'], 'hidden UI or missing events')
        for event in sample['events']:
            require(type(event['seq']) is int and event['seq'] == sequence + 1, 'lost or duplicate raw event')
            require(type(event['atMs']) in (int, float) and math.isfinite(event['atMs']), 'invalid event clock')
            if events:
                require(event['atMs'] >= events[-1]['atMs'], 'reversed event time')
            sequence += 1
            events.append(event)
        require(sequence == sample['eventCount'], 'incomplete snapshot event ledger')
    require(samples[0]['mono_after_ns'] <= start_ns and samples[-1]['mono_before_ns'] >= end_ns,
            'snapshots do not enclose native interval')
    attempts, headers, completions, active, active_since, spans = {}, {}, {}, False, None, []
    timers, ticks, attempted_ticks = {}, {}, set()
    for event in events:
        kind, at = event['event'], event['atMs'] * 1_000_000
        if kind == 'polling':
            require(type(event['active']) is bool and event['active'] != active, 'invalid polling transition')
            if event['active']:
                active_since = at
            else:
                spans.append((active_since, at))
            active = event['active']
        elif kind == 'timer':
            require(event['delay'] == 5000 and event['timer'] not in timers, 'native timer registration mismatch')
            timers[event['timer']] = at
        elif kind == 'tick':
            require(event['timer'] in timers and event['tick'] not in ticks, 'unregistered or duplicate timer tick')
            ticks[event['tick']] = event
        elif kind in ('attempt', 'headers', 'complete'):
            key = event['id']
            require(type(key) is int and key > 0, 'invalid attempt identity')
            if kind == 'attempt':
                require(active and key not in attempts, 'attempt outside active UI or duplicate')
                require(event['tick'] in ticks and event['tick'] not in attempted_ticks,
                        'attempt missing its native timer tick')
                attempted_ticks.add(event['tick'])
                attempts[key] = at
            elif kind == 'headers':
                require(key in attempts and key not in headers and type(event['status']) is int
                        and 200 <= event['status'] < 300, 'failed status or unmatched headers')
                headers[key] = at
            else:
                require(key in headers and key not in completions and event['failed'] is False,
                        'failed transport or incomplete native body')
                completions[key] = at
        else:
            raise ValueError('UI continuity: unknown native event')
    if active:
        spans.append((active_since, samples[-1]['nowMs'] * 1_000_000))
    require(any(lo + offset_hi <= start_ns and hi + offset_lo >= end_ns for lo, hi in spans),
            'requested polling paused during exposure')
    relevant = sorted(at for at in attempts.values()
                      if at + offset_lo <= end_ns and at + offset_hi >= start_ns)
    require(relevant and relevant[0] + offset_hi <= start_ns + 20 * NS
            and relevant[-1] + offset_lo >= end_ns - 20 * NS,
            'missing beginning or final native UI attempt')
    require(all(right - left <= 20 * NS for left, right in zip(relevant, relevant[1:])),
            'native UI made no progress for twenty seconds')
    for key, event in ticks.items():
        at = event['atMs'] * 1_000_000
        if at + offset_hi < start_ns or at + offset_lo > end_ns:
            continue
        require(event['visibility'] == 'visible' and event['running'] is True
                and event['connected'] is True and type(event['busy']) is bool,
                'native timer observed hidden, paused or locked UI')
        require(event['busy'] or key in attempted_ticks, 'eligible native timer callback lost its request')
    for key, at in attempts.items():
        if at + offset_hi < start_ns or at + offset_lo > end_ns:
            continue
        require(key in completions and attempts[key] <= headers[key] <= completions[key]
                and completions[key] - at <= 15 * NS, 'native UI body deadline or completion missing')
    gaps = [(right-left)/1_000_000 for left, right in zip(relevant, relevant[1:])]
    return {'attempts': len(relevant), 'clock_offset_ns': [offset_lo, offset_hi],
            'attempt_gap_ms': gaps, 'max_attempt_gap_ms': max(gaps) if gaps else None,
            'raw_events': sequence, 'document_session': list(identity)}
