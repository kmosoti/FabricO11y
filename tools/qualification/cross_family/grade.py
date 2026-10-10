"""Independent adapter joins; missing evidence never becomes a passing gate."""
from __future__ import annotations
import base64
import copy
import hashlib
import json
import math
from pathlib import Path
import re
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import delivery_oracle
import query_oracle


def nearest_rank(values, quantile=.99):
    if not values:
        return None
    ordered = sorted(values)
    return ordered[math.ceil(quantile * len(ordered)) - 1]


def native_transcript(source, output):
    state = [row for row in source if row['type'] == 'node_state']
    if len(state) != 1:
        raise ValueError('one exact native spool identity required')
    node, generation = state[0]['node_id'], state[0]['generation']
    by_sequence = {row['sequence']: row for row in source if row['type'] == 'source'}
    if len(by_sequence) != sum(row['type'] == 'source' for row in source):
        raise ValueError('duplicate source sequence in native spool')
    result = list(by_sequence.values())
    for sequence, checksum, kind, through in re.findall(r'delivery sequence=(\d+) sha256=([0-9a-f]{64}) status=(\w+)(?: committed_through=(\d+))? elapsed_us=\d+', output):
        sequence = int(sequence)
        if sequence not in by_sequence:
            raise ValueError('native source bytes reclaimed or missing: incomplete custody evidence')
        body = by_sequence[sequence]['bytes']
        if hashlib.sha256(base64.b64decode(body, validate=True)).hexdigest() != checksum:
            raise ValueError('actual sent hash differs from independently recovered native source bytes')
        identity = {'node_id': node, 'generation': generation, 'sequence': sequence}
        result.append({'type': 'attempt', **identity, 'bytes': body, 'injected_conflict': False})
        response = {'type': 'response', **identity, 'kind': kind}
        if kind == 'ack':
            response['committed_through'] = int(through)
        result.append(response)
    result.extend(state)
    return result


def custody_grade(transcript, recovered):
    complete = [*transcript, *recovered, {'type': 'end'}]
    verdict = delivery_oracle.check(json.dumps(row) for row in complete)
    negatives = []
    for index, row in enumerate(recovered):
        if any(r['type'] == 'response' and r.get('kind') == 'ack'
               and r['node_id'] == row['node_id'] and r['generation'] == row['generation']
               and r['committed_through'] >= row['sequence'] for r in transcript):
            altered = [*transcript, *recovered[:index], *recovered[index+1:], {'type': 'end'}]
            defect = delivery_oracle.check(json.dumps(r) for r in altered)
            negatives = [v for v in defect.violations if v['rule'] == 'ACKED-DURABLE']
            break
    return {'passed': verdict.passed and bool(negatives), 'counts': verdict.counts,
            'violations': verdict.violations,
            'drop_recovered_negative_detected': bool(negatives)}


def query_grade(records, queries, groups=None):
    verdicts = []
    negative = False
    for query, pages in queries:
        if groups is not None:
            from snapshot import select
            selected = select(groups, pages)
        else:
            selected = records
        verdict = query_oracle.check(selected, query, pages)
        verdicts.append(verdict)
        if not negative and pages and pages[0]['rows']:
            changed = copy.deepcopy(pages)
            row = changed[0]['rows'][0]
            if 'body' in row:
                row['body'] += '-injected-defect'
            elif 'value' in row:
                row['value'] += 1
            else:
                row['name'] += '-injected-defect'
            rejected = query_oracle.check(selected, query, changed)
            negative = not rejected['passed'] and any(v['rule'] == 'ROW-CONTENT' for v in rejected['violations'])
    return {'passed': bool(verdicts) and all(v['passed'] for v in verdicts) and negative,
            'verdicts': verdicts, 'mismatched_row_negative_detected': negative}


def measure_and_grade(server, edge, call, config, result, data_port):
    from measure import collect
    from populations import samples, row_key, window
    import release_timing
    import timing
    from snapshot import select
    observation = collect(server, edge, call, config, result, data_port)
    gates = {}
    ends = [o for o in observation['offers'] if o['type']=='end']
    logs = [o for o in observation['offers'] if o['type']=='log']
    offered_traces = [o for o in observation['offers'] if o['type']=='trace_offer']
    metric_commits = [o for o in observation['offers'] if o['type']=='metric_committed']
    gates['producer'] = {'passed': len(ends)==1 and not ends[0]['failures'] and len(logs)==390
                         and len(offered_traces)==1950 and len(metric_commits)==13,
                         'logs':len(logs),'traces':len(offered_traces),'controlled_batches':len(metric_commits),
                         'controlled_points':sum(m['points'] for m in metric_commits), 'end':ends}
    if not gates['producer']['passed']:
        return gates
    combined = []
    for role in ('edge', 'server'):
        combined.extend(native_transcript(observation['sources'][role], observation['outputs'][role]))
    combined.extend(observation['controlled'])
    gates['custody'] = custody_grade(combined, observation['recovered'])
    if not gates['custody']['passed']:
        return gates
    for index, queries in enumerate(observation['fixed_queries']):
        gates['query_phase_' + str(index)] = query_grade(observation['records'], queries, observation['groups'])
    rows, populations, batches = samples(observation, server.boot_id, edge.boot_id, config['log'])
    expected_log_offsets = {o['offset_start'] for o in logs}
    actual_logs, actual_spans = [], []
    for source in observation['sources']['edge']:
        if source['type']=='source':
            record = query_oracle.materialize_record('edge', 0, base64.b64decode(source['bytes'], validate=True))
            actual_logs.extend(r for r in record.log_rows if r['attributes'].get('log.file.path')==config['log'])
            actual_spans.extend(record.span_rows)
    wanted_spans = {(r['trace_id'],r['span_id']) for o in offered_traces for r in o['rows']}
    gates['independent_offers'] = {'passed':len(actual_logs)==390 and len(actual_spans)==5850
                    and {int(r['attributes']['log.file.offset.start']) for r in actual_logs}==expected_log_offsets
                    and {(r['trace_id'],r['span_id']) for r in actual_spans}==wanted_spans,
                    'retained_logs':len(actual_logs),'retained_spans':len(actual_spans),
                    'offered_logs':390,'offered_spans':5850}
    seen = {}
    for query in observation['queries']:
        body, page = query['query'], query['answer']
        # These rolling visibility reads may omit older records outside their
        # explicit bounds; within that declared query snapshot the unchanged
        # oracle still checks every row, value and envelope field.
        verdict = query_oracle.check(select(observation['groups'], [page]), body, [page])
        if not verdict['passed']:
            gates['visibility_query_oracle'] = {'passed': False, 'verdict': verdict, 'query': body}
            return gates
        finish = timing.interval(query['after']['boot_id'], query['before']['monotonic_before_ns'],
                                 query['after']['monotonic_after_ns'])
        for row in page['rows']:
            key = row_key(body['kind'], row)
            if key not in rows:
                raise ValueError('visible row has no independently retained source timing')
            if key not in seen:
                seen[key] = finish
    for population, values in populations.items():
        measured = [v for v in values if v['measured']]
        ack = [v['ack_duration']['upper_ns'] / 1e9 for v in measured]
        visible, missing = [], []
        for value in measured:
            key = tuple(value['key'])
            if key not in seen:
                missing.append(value['key'])
                continue
            visible.append(timing.elapsed(value['start'], seen[key])['upper_ns'] / 1e9)
        ack99, visibility99 = nearest_rank(ack), nearest_rank(visible)
        gates['timing_' + population] = {'passed': bool(measured) and not missing
                        and ack99 is not None and ack99 <= 1 and visibility99 is not None and visibility99 <= 5,
                        'measured_rows': len(measured), 'all_rows': len(values), 'censored_visibility': len(missing),
                        'ack_p99_upper_s': ack99, 'visibility_p99_upper_s': visibility99,
                        'missing_visibility_keys': missing, 'samples': values,
                        'boundary': {'offered_edge': 'producer creation', 'controlled_metrics': 'producer creation',
                                     'native_host_metrics': 'native collection start',
                                     'companion_diagnostics': 'native source acceptance; earlier creation unmeasured'}[population]}
    for role, entries in batches.items():
        start, end = window(observation['begin'], observation['correlations'], 'edge' if role == 'controlled' else role)
        grade = release_timing.population(entries, start, end, rule='clearing-v2')
        # Cross-family revision 1 explicitly sampled every second. Keep those
        # original means as diagnostics, alongside reference v2 full unions.
        sampled = []
        for at in range(start, end, 10**9):
            pending = [e for e in entries if e['created_ns'] <= at < e['ack_ns']]
            sampled.append({'at_ns': at, 'count': len(pending), 'bytes': sum(e['bytes'] for e in pending)})
        first, last = [s for s in sampled if s['at_ns'] < start+30*10**9], [s for s in sampled if s['at_ns'] >= end-30*10**9]
        means = {k: {'first': sum(s[k] for s in first)/len(first), 'last': sum(s[k] for s in last)/len(last)} for k in ('count', 'bytes')}
        grade.update(entries=entries, original_one_second_samples=sampled, original_one_second_means=means,
                     original_one_second_nonincreasing=all(m['last'] <= m['first'] for m in means.values()),
                     phase_clock_bounds={'from_ns': start, 'to_ns': end},
                     interpretation='clearing-v2 finite bounded debt; no stationarity or nonincreasing occupancy claim')
        gates['backlog_' + role] = grade
    for role, cap in [('edge', 64*1024**2), ('server', 2*1024**3)]:
        start, end = window(observation['begin'], observation['correlations'], role)
        selected = [s for s in observation['resources'][role] if start <= s['before_ns'] < end]
        valid = bool(selected) and all(s['boot_id'] == (edge.boot_id if role == 'edge' else server.boot_id)
            and type(s['resource'].get('rss_bytes')) is int for s in selected)
        peak = max((s['resource']['rss_bytes'] for s in selected if type(s['resource'].get('rss_bytes')) is int), default=None)
        oom = []
        for s in selected:
            value = s['resource']['memory.events']
            if not isinstance(value, str):
                valid = False
                continue
            events = dict(line.split() for line in value.splitlines())
            oom.append(int(events['oom']) + int(events['oom_kill']))
            if s['resource']['memory.swap.max'] != '0':
                valid = False
        cadence_complete = valid and len(selected) >= 119 and all(b['before_ns']-a['before_ns'] <= 2*10**9
                                                for a,b in zip(selected, selected[1:]))
        gates['resources_' + role] = {'passed': cadence_complete and peak is not None and peak <= cap and bool(oom) and max(oom) == 0,
                                      'rss_peak_bytes': peak, 'cap_bytes': cap, 'samples': selected,
                                      'complete': cadence_complete, 'oom_events': max(oom, default=None)}
    phases = observation['phases']
    updates, applied = [p for p in phases if p['type']=='config_update'], [p for p in phases if p['type']=='config_applied']
    delay = None
    if len(updates) == len(applied) == 1:
        delay = (applied[0]['clock']['monotonic_after_ns']-updates[0]['before']['monotonic_before_ns'])/1e9
    gates['configuration'] = {'passed': delay is not None and 0 <= delay <= 30, 'apply_upper_s': delay}
    drops = [p for p in phases if p['type']=='outage_end']
    counters = [item['counter'] for p in drops for item in p['firewall_counters']['nftables']
                if 'rule' in item for item in item['rule'].get('expr', []) if 'counter' in item]
    gates['data_outage'] = {'passed': len(drops)==1 and any(c['packets']>0 and c['bytes']>0 for c in counters),
                            'counters': counters, 'phases': [p for p in phases if p['type'].startswith('outage_')]}
    drains = [p for p in phases if p['type']=='drain']
    gates['drain'] = {'passed': bool(drains) and all(int(v['acked_through'])==int(v['next_sequence'])-1
                                                   for v in drains[-1]['views'].values()), 'last': drains[-1] if drains else None}
    (result / 'gates.json').write_text(json.dumps(gates, indent=2)+'\n')
    return gates
