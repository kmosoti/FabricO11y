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


def query_grade(records, queries):
    verdicts = []
    negative = False
    for query, pages in queries:
        verdict = query_oracle.check(records, query, pages)
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
            rejected = query_oracle.check(records, query, changed)
            negative = not rejected['passed'] and any(v['rule'] == 'ROW-CONTENT' for v in rejected['violations'])
    return {'passed': bool(verdicts) and all(v['passed'] for v in verdicts) and negative,
            'verdicts': verdicts, 'mismatched_row_negative_detected': negative}


def measure_and_grade(server, edge, call, config, result, data_port):
    raise RuntimeError('cross-family measurement adapter not yet complete; no cell can be reported passed')
