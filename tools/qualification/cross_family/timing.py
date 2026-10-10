"""Conservative same-boot clock joins, independent of application transitions."""
import re

LINE = re.compile(r'^timing process_id=(\d+) node_id=([0-9a-f]{32}) generation=(\d+) sequence=(\d+) stage=(\w+) unix_ns=(\d+|unmeasured) boot_monotonic_ns=(\d+|unmeasured) monotonic_before_ns=(\d+) monotonic_after_ns=(\d+)$')


def events(text, boot_id):
    if not boot_id:
        raise ValueError('explicit guest boot identity required')
    answer = []
    for line in text.splitlines():
        if line.startswith('timing dropped_events='):
            if int(line.split('=', 1)[1]) != 0:
                raise ValueError('lost timing events: population is incomplete')
            continue
        if not line.startswith('timing '):
            continue
        match = LINE.fullmatch(line)
        if not match:
            raise ValueError('malformed timing observation')
        pid, node, generation, sequence, stage, wall, boot, before, after = match.groups()
        before, after = int(before), int(after)
        if after < before or boot == 'unmeasured':
            raise ValueError('missing or inconsistent monotonic clock observation')
        # The shared clock read is inside the process-local sampling bracket.
        # Expand by its full width; do not pretend the sample is exact.
        width = after - before
        point = int(boot)
        answer.append({'boot_id': boot_id, 'process_id': int(pid), 'node_id': node,
                       'generation': int(generation), 'sequence': int(sequence), 'stage': stage,
                       'lower_ns': max(0, point - width), 'upper_ns': point + width,
                       'sample_width_ns': width, 'unix_ns': None if wall == 'unmeasured' else int(wall)})
    return answer


def interval(boot_id, lower_ns, upper_ns):
    if not boot_id or type(lower_ns) is not int or type(upper_ns) is not int or lower_ns < 0 or upper_ns < lower_ns:
        raise ValueError('invalid measured clock bracket')
    return {'boot_id': boot_id, 'lower_ns': lower_ns, 'upper_ns': upper_ns}


def elapsed(start, finish):
    if start['boot_id'] != finish['boot_id']:
        raise ValueError('different guest boots cannot be joined by monotonic time')
    lower = finish['lower_ns'] - start['upper_ns']
    upper = finish['upper_ns'] - start['lower_ns']
    if upper < 0:
        raise ValueError('completion precedes creation even after clock uncertainty')
    return {'lower_ns': max(0, lower), 'upper_ns': upper,
            'uncertainty_ns': upper - lower}


def acknowledged_samples(observed, valid_acks, creation):
    """One valid durable ACK per required sequence, including retries/wait time.

    The caller independently validates ACK custody before supplying identities.
    Creation brackets must come from the declared workload population, rather
    than treating send-start or an HTTP response as creation.
    """
    rows = []
    for identity in creation:
        candidates = [e for e in observed if e['stage'] == 'answer_received'
                      and (e['node_id'], e['generation'], e['sequence']) == identity
                      and (e['node_id'], e['generation'], e['sequence'], e['lower_ns'], e['upper_ns']) in valid_acks]
        if not candidates:
            raise ValueError('required source has no independently validated ACK timing')
        finish = min(candidates, key=lambda e: e['upper_ns'])
        rows.append({'identity': list(identity), **elapsed(creation[identity], finish)})
    return rows
