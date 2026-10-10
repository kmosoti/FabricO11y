"""Source-bound four-population timing and full accepted Batch intervals."""
import base64
import hashlib
import re
import sys
from pathlib import Path
import timing
import query_oracle
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import release_timing


def row_key(kind, row):
    return (kind, row['node_id'], row['sequence'], row['index'])


def window(begin, correlations, role):
    if role == 'edge':
        return begin['monotonic_ns'] + 15*10**9, begin['monotonic_ns'] + 135*10**9
    # Each guest sample occurred inside its measured host SSH round trip.
    # Compose those interval offsets, without assuming wall clocks agree.
    bounds = []
    for pair in correlations:
        edge, server = pair['edge'], pair['server']
        edge_host_low = edge['host_request_before_ns'] - edge['monotonic_after_ns']
        edge_host_high = edge['host_request_after_ns'] - edge['monotonic_before_ns']
        server_host_low = server['host_request_before_ns'] - server['monotonic_after_ns']
        server_host_high = server['host_request_after_ns'] - server['monotonic_before_ns']
        bounds.append((edge_host_low-server_host_high, edge_host_high-server_host_low))
    low = begin['monotonic_ns'] + min(b[0] for b in bounds)
    high = begin['monotonic_ns'] + max(b[1] for b in bounds)
    # Grade the larger conservative interval. Passing it also covers the
    # intended 120-second interval for every admissible phase mapping.
    return low + 15*10**9, high + 135*10**9


def native(sources, output, boot):
    parsed = timing.events(output, boot)
    by_sequence = {r['sequence']: r for r in sources if r['type'] == 'source'}
    if len(by_sequence) != sum(r['type'] == 'source' for r in sources):
        raise ValueError('duplicate independent native source')
    states = [r for r in sources if r['type'] == 'node_state']
    if len(states) != 1:
        raise ValueError('one exact native state required')
    stream = (states[0]['node_id'], states[0]['generation'])
    if any((e['node_id'], e['generation']) != stream or e['sequence'] not in by_sequence for e in parsed):
        raise ValueError('native timing identity/source changed')
    deliveries = {}
    for line in output.splitlines():
        if line.startswith('delivery '):
            values = dict(part.split('=', 1) for part in line.split()[1:])
            deliveries.setdefault(int(values['sequence']), []).append(values)
    answer = {}
    for seq, source in by_sequence.items():
        stages = {}
        for event in parsed:
            if event['sequence'] == seq:
                stages.setdefault(event['stage'], []).append(event)
        accepted = stages.get('sources_accepted', stages.get('traces_accepted', []))
        committed = stages.get('spool_committed', [])
        sends, responses = stages.get('send_started', []), stages.get('answer_received', [])
        attempts = deliveries.get(seq, [])
        if len(accepted) != 1 or len(committed) != 1 or len(sends) != len(responses) or len(responses) != len(attempts):
            raise ValueError('source/commit/attempt clock population incomplete')
        ack = None
        raw = base64.b64decode(source['bytes'], validate=True)
        for request, response, attempt in zip(sends, responses, attempts):
            timing.elapsed(request, response)
            if attempt['sha256'] != hashlib.sha256(raw).hexdigest():
                raise ValueError('actual attempted bytes differ from independent source')
            if attempt['status'] == 'ack':
                if int(attempt['committed_through']) != seq:
                    raise ValueError('invalid printed ACK cursor')
                if ack is None:
                    ack = response
        if ack is None:
            raise ValueError('required native source has censored ACK')
        timing.elapsed(accepted[0], committed[0])
        collection = stages.get('collection_started', [])
        if len(collection) > 1:
            raise ValueError('ambiguous native collection boundary')
        answer[seq] = {'acceptance': accepted[0], 'commit': committed[0], 'ack': ack,
                       'collection': collection[0] if collection else None, 'bytes': len(raw),
                       'attempts': len(attempts), 'retries': sum(a['status'] != 'ack' for a in attempts),
                       'successful_attempt_upper_ns': timing.elapsed(sends[responses.index(ack)], ack)['upper_ns']}
    return answer


def samples(observation, server_boot, edge_boot, source_path):
    begin = observation['begin']
    if begin['boot_id'] != edge_boot:
        raise ValueError('producer boot identity changed')
    offers = observation['offers']
    logs = {o['offset_start']: o for o in offers if o['type'] == 'log'}
    traces = {(r['trace_id'], r['span_id']): o for o in offers if o['type'] == 'trace_offer' for r in o['rows']}
    metric_sources = {o['sequence']: o for o in offers if o['type'] == 'metric_committed'}
    metric_acks = {o['sequence']: o for o in offers if o['type'] == 'metric_ack'}
    controls = [s for s in observation['controlled'] if s['type'] == 'source']
    rows, batches = {}, {'edge': [], 'server': [], 'controlled': []}
    populations = {'offered_edge': [], 'controlled_metrics': [], 'native_host_metrics': [], 'companion_diagnostics': []}
    def add(kind, row, start, ack, population, role):
        key = row_key(kind, row)
        if key in rows:
            raise ValueError('duplicate independently recovered row identity')
        lower, upper = window(begin, observation['correlations'], role)
        measured = start['upper_ns'] >= lower and start['lower_ns'] < upper
        value = {'key': list(key), 'role': role, 'population': population, 'start': start, 'ack': ack,
                 'measured': measured, 'ack_duration': timing.elapsed(start, ack)}
        rows[key] = value
        populations[population].append(value)
    for role, boot, label in [('edge', edge_boot, 'edge'), ('server', server_boot, 'fabric-server-self')]:
        joined = native(observation['sources'][role], observation['outputs'][role], boot)
        for source in observation['sources'][role]:
            if source['type'] != 'source':
                continue
            seq = source['sequence']
            events = joined[seq]
            record = query_oracle.materialize_record(label, 0, base64.b64decode(source['bytes'], validate=True))
            starts = []
            for row in record.log_rows:
                if role == 'edge':
                    if row['attributes'].get('log.file.path') != source_path:
                        raise ValueError('unexpected unregistered edge log source')
                    offer = logs[int(row['attributes']['log.file.offset.start'])]
                    if offer['body'] != row['body']:
                        raise ValueError('exact offered log bytes changed')
                    start = timing.interval(boot, offer['before_ns'], offer['after_ns'])
                    population = 'offered_edge'
                else:
                    start, population = events['acceptance'], 'companion_diagnostics'
                starts.append(start)
                add('logs', row, start, events['ack'], population, role)
            for row in record.span_rows:
                if role != 'edge':
                    raise ValueError('unregistered companion trace producer')
                offer = traces[(row['trace_id'], row['span_id'])]
                expected = next(r for r in offer['rows'] if r['span_id'] == row['span_id'])
                if any(row[k] != v for k, v in expected.items()):
                    raise ValueError('offered span identity, parentage or content changed')
                start = timing.interval(boot, offer['before_ns'], offer['before_ns'])
                starts.append(start)
                add('spans', row, start, events['ack'], 'offered_edge', role)
            for row in record.metric_points:
                if events['collection'] is None:
                    raise ValueError('native metric lacks collection-start clock')
                start = events['collection']
                starts.append(start)
                add('metrics', row, start, events['ack'], 'native_host_metrics', role)
            if not starts:
                starts = [events['acceptance']]
            batches[role].append({'key': [source['node_id'], source['generation'], seq],
                                  'created_ns': min(s['lower_ns'] for s in starts),
                                  'ack_ns': events['ack']['upper_ns'], 'bytes': events['bytes'],
                                  'attempts': events['attempts'], 'retries': events['retries'],
                                  'successful_attempt_upper_ns': events['successful_attempt_upper_ns']})
    for source in controls:
        seq = source['sequence']
        made, ack = metric_sources[seq], metric_acks.get(seq)
        if ack is None:
            raise ValueError('required controlled metric has censored ACK')
        start = timing.interval(edge_boot, made['scheduled_monotonic_ns'], made['monotonic_ns'])
        finish = timing.interval(edge_boot, ack['before_ns'], ack['after_ns'])
        record = query_oracle.materialize_record('controlled', 0, base64.b64decode(source['bytes'], validate=True))
        if (len(record.metric_points) != 32
                or {r['attributes'].get('point_id') for r in record.metric_points} != {str(i) for i in range(32)}):
            raise ValueError('controlled population differs from registered 32 points')
        for row in record.metric_points:
            point = int(row['attributes']['point_id'])
            if (not 0 <= point < 32 or row['value'] != made['k'] * (point + 1)
                    or row['start_ns'] != begin['unix_ns'] or row['time_ns'] != made['observed_ns']
                    or not row['monotonic'] or row['kind'] != 'sum'):
                raise ValueError('controlled cumulative metric values or identity changed')
            add('metrics', row, start, finish, 'controlled_metrics', 'edge')
        batches['controlled'].append({'key': [source['node_id'], source['generation'], seq],
                                     'created_ns': start['lower_ns'], 'ack_ns': finish['upper_ns'],
                                     'bytes': len(base64.b64decode(source['bytes'], validate=True))})
    return rows, populations, batches
