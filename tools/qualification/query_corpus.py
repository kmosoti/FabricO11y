"""Independent all-reader corpus with native-timing-bounded companion snapshots."""
import base64

import query_oracle as oracle

COMPANION_LABEL = 'fabric-server-self'


def identity(record):
    batch = oracle.decode_batch(base64.b64decode(record['bytes'], validate=True))
    return batch['node_id'].hex(), batch['generation'], batch['sequence']


def join_companion(sources, recovered):
    """Take source bytes from Spool; recovery supplies only verified commit metadata."""
    if not sources:
        raise ValueError('missing independent companion sources')
    keys = [(r['node_id'], r['generation'], r['sequence']) for r in sources]
    stream = keys[0][:2]
    if any(k[:2] != stream for k in keys) or [k[2] for k in keys] != list(range(1, len(keys)+1)):
        raise ValueError('independent companion source prefix is invalid')
    by_key = dict(zip(keys, sources))
    for key, source in by_key.items():
        if identity(source) != key:
            raise ValueError('companion source identity differs from its original bytes')
        if oracle.decode_batch(base64.b64decode(source['bytes'], validate=True))['gaps']:
            raise ValueError('companion gap violates zero-gap corpus admission')
    joined = {}
    for record in recovered:
        key = identity(record)
        if key not in by_key or key in joined or record['label'] != COMPANION_LABEL:
            raise ValueError('unknown, duplicate or mislabeled companion recovery')
        source = by_key[key]
        if base64.b64decode(record['bytes'], validate=True) != base64.b64decode(source['bytes'], validate=True):
            raise ValueError('companion recovery bytes differ from independent Spool')
        received = record['received_ns']
        if type(received) is not int or received <= 0:
            raise ValueError('invalid companion recovery commit metadata')
        joined[key] = {'label': COMPANION_LABEL, 'bytes': source['bytes'], 'received_ns': received}
    ordered = sorted(joined)
    if not ordered or [k[2] for k in ordered] != list(range(1, len(ordered)+1)):
        raise ValueError('missing companion recovered prefix')
    return [joined[key] for key in ordered]


class SnapshotCorpus:
    def __init__(self, fixture, companion, timing, labels, prepare):
        self.labels = set(labels) | {COMPANION_LABEL}
        self.fixture = prepare(fixture, self.labels)
        self.companion = prepare(companion, self.labels)
        self.recovered_prefix = len(companion)
        self.timing = sorted(timing, key=lambda event: event['key'][2])
        keys = [tuple(event['key']) for event in self.timing]
        if not keys or len({k[:2] for k in keys}) != 1 or [k[2] for k in keys] != list(range(1, len(keys)+1)):
            raise ValueError('missing or inconsistent companion timing prefix')
        if any(identity(record) != keys[index] for index, record in enumerate(companion)):
            raise ValueError('companion timing identity differs from independently joined source')
        previous = -1
        for event in self.timing:
            created, ack = event['created_ns'], event['ack_ns']
            if type(created) is not int or created < previous or (ack is not None and (type(ack) is not int or ack < created)):
                raise ValueError('invalid same-boot companion timing order')
            previous = created
        self.projected_sequences = {
            kind: [(identity(record)[2], record) for record in records]
            for kind, records in self.companion.items()}

    def bounds(self, attempted, completed):
        if type(attempted) is not int or type(completed) is not int or not 0 <= attempted <= completed:
            raise ValueError('invalid first-page snapshot timing')
        lower = max((e['key'][2] for e in self.timing
                     if e['ack_ns'] is not None and e['ack_ns'] <= attempted), default=0)
        upper = min(self.recovered_prefix, max((e['key'][2] for e in self.timing
                    if e['created_ns'] <= completed), default=0))
        if lower > upper or upper-lower+1 > 32:
            raise ValueError('independent companion snapshot bounds unavailable or too wide')
        return lower, upper

    def grade(self, query, pages, attempted, completed, grader):
        lower, upper = self.bounds(attempted, completed)
        candidates = []
        for prefix in range(lower, upper+1):
            records = self.fixture[query['kind']] + [record for sequence, record
                in self.projected_sequences[query['kind']] if sequence <= prefix]
            verdict = grader([], query, pages, self.labels, prepared={query['kind']: records})
            candidates.append({'prefix': prefix, 'verdict': verdict})
            if verdict['passed']:
                return dict(verdict, snapshot_prefix={'lower': lower, 'upper': upper, 'selected': prefix})
        return {'passed': False, 'violations': [{'rule': 'NO-INDEPENDENT-SNAPSHOT',
                'detail': 'no independently admitted companion prefix matches the unchanged oracle'}],
                'snapshot_prefix': {'lower': lower, 'upper': upper, 'selected': None},
                'snapshot_candidates': candidates}
