"""Independent raw-journal/query joins for the registered owner walkthrough.

No application codec is imported. Existing snapshot and query oracles retain
all of their framing, row, page, completeness and freshness checks.
"""
import base64
import hashlib
import json

import query_oracle
from cross_family.snapshot import decode
from release_query import grade
import delivery_oracle
import soak_companion


def require(value, reason):
    if not value:
        raise ValueError(reason)


def verify_retained_resident_assertion(value, origin, rp_id):
    require(type(value) is dict and not value.get('error')
            and value.get('one_shot') is True and value.get('original_allowlist_count', 0) > 0
            and value.get('rp_id') == rp_id and value.get('verification_unchanged') is True
            and value.get('challenge_matches') is True and value.get('origin') == origin
            and value.get('type') == 'webauthn.get'
            and value.get('rp_hash') == hashlib.sha256(rp_id.encode()).hexdigest()
            and value.get('up') is True and value.get('uv') is True,
            'retained native assertion lacks unchanged challenge/origin/RP/UP/UV evidence')


def bounded_edge_records(records, proof):
    before, after = proof['before'], proof['after']
    require(before['boot_id'] == after['boot_id'] and before['boot_id'], 'edge boot changed')
    require((before['node_id'], before['generation']) == (after['node_id'], after['generation']),
            'edge source identity changed')
    require(before['finished_ns'] <= proof['attempted_ns'] <= proof['completed_ns'] <= after['started_ns'],
            'edge snapshot observations do not surround first page')
    lower, upper = before['acked_through'], after['next_sequence'] - 1
    require(type(lower) is int and type(upper) is int and 1 <= lower <= upper
            and upper - lower < 32, 'edge snapshot bounds unavailable or too wide')
    selected = []
    for record in records:
        if record['label'] != 'edge':
            continue
        batch = query_oracle.decode_batch(base64.b64decode(record['bytes'], validate=True))
        require((batch['node_id'].hex(), batch['generation']) == (before['node_id'], before['generation']),
                'recovered edge identity differs from observed source')
        require(not batch['gaps'], 'edge gap violates scoped zero-gap admission')
        selected.append((batch['sequence'], record))
    require([sequence for sequence, _ in selected] == list(range(1, len(selected) + 1)),
            'independent recovered edge prefix missing or reordered')
    require(len(selected) >= lower, 'acknowledged edge prefix absent from recovery')
    return lower, min(upper, len(selected)), selected


def grade_custody(spools, replay):
    """Spool sources and durable cursors; no invented wire attempts or responses."""
    observations = [soak_companion.inspect_records(rows) for rows in spools]
    sources = [record for observation in observations for record in observation['projected_sources']]
    recovered = []
    for record in replay:
        raw = base64.b64decode(record['bytes'], validate=True)
        batch = query_oracle.decode_batch(raw)
        recovered.append(dict(type='recovered', node_id=batch['node_id'].hex(),
                              generation=batch['generation'], sequence=batch['sequence'],
                              bytes=base64.b64encode(hashlib.sha256(raw).digest()).decode()))
    ledger = sources + recovered + [{'type': 'end'}]
    verdict = delivery_oracle.check(json.dumps(record) for record in ledger).to_json()
    require(verdict['passed'], 'independent delivery oracle rejected walkthrough custody')
    return {'delivery_oracle': verdict,
            'durable_cursors': [soak_companion.validate_recovery(observation, recovered)
                               for observation in observations], 'wire_attempts_observed': False}


def grade_queries(journals, replay, queries, source_log):
    require(isinstance(journals, list) and journals, 'complete original stopped journals required')
    names = [item['file'] for item in journals]
    require(len(names) == len(set(names)), 'duplicate original journal member')
    groups = sorted((group for item in journals
                     for group in decode(base64.b64decode(item['bytes'], validate=True))),
                    key=lambda group: group['group'])
    require([group['group'] for group in groups] == list(range(1, len(groups) + 1)),
            'complete original journal prefix required')
    require([record for group in groups for record in group['records']] == replay,
            'independent raw journal differs from exact replay helper')
    verdicts = []
    seen = set()
    for query, pages, proof in queries:
        require(query.get('node') == 'edge', 'walkthrough query scope differs')
        lower, upper, records = bounded_edge_records(replay, proof)
        attempts = []
        for prefix in range(lower, upper + 1):
            verdict = grade([record for sequence, record in records if sequence <= prefix],
                            query, pages, {'edge'})
            attempts.append({'prefix': prefix, 'verdict': verdict})
            if verdict['passed']:
                break
        require(attempts and attempts[-1]['verdict']['passed'],
                'independent query oracle rejected every bounded walkthrough snapshot')
        verdict = dict(attempts[-1]['verdict'], snapshot_bounds={'lower': lower, 'upper': upper},
                       snapshot_attempts=attempts)
        rows = [row for page in pages for row in page['rows']]
        require(rows, 'walkthrough signal query is empty')
        if query.get('node') == 'edge':
            seen.add(query['kind'])
            if query['kind'] == 'logs':
                expected = source_log.decode('utf-8').splitlines()
                require(len(expected) == 1 and sum(row['body'] == expected[0] for row in rows) == 1,
                        'original source log lacks one exact queried row')
            if query['kind'] == 'spans':
                require(sum(row['name'] == 'walkthrough-span' for row in rows) == 1,
                        'real guide exporter span missing or duplicated')
        verdicts.append(verdict)
    require(seen == {'logs', 'metrics', 'spans'}, 'all three received edge signals required')
    return {'verdicts': verdicts, 'raw_groups': len(groups),
            'raw_sha256': {item['file']: hashlib.sha256(base64.b64decode(item['bytes'], validate=True)).hexdigest()
                           for item in journals}}


def completed(record, expected_binding, phase_names):
    """Do not allow a partial transcript or a foreign candidate to become complete."""
    require(record.get('binding') == expected_binding, 'walkthrough candidate/guide/harness binding differs')
    phases = record.get('phases', [])
    require(phases and {row.get('name') for row in phases} == set(phase_names), 'walkthrough phase omitted')
    totals = {name: 0 for name in phase_names}
    previous = phases[0]['start_ns']
    for row in phases:
        require(row.get('exit') == 0 and type(row.get('end_ns')) is int
                and previous <= row['start_ns'] <= row['end_ns'] <= row['deadline_ns'],
                'walkthrough phase incomplete or exceeded allocation')
        totals[row['name']] += row['end_ns'] - row['start_ns']
        previous = row['end_ns']
    require(all(totals[name] <= seconds * 10**9 for name, seconds in phase_names.items())
            and previous - phases[0]['start_ns'] <= 1800 * 10**9,
            'walkthrough accumulated or overall deadline exceeded')
    require(record.get('revoked_edge_http') == 401, 'revoked edge requires specific HTTP401')
    require(record.get('old_owner_denied') is True and record.get('stale_session_http') == 401,
            'old owner or stale session authority not denied')
    require(record.get('old_owner_http') == 403 and record.get('old_owner_error') == 'access denied',
            'old owner requires exact native403/access-denied evidence')
    verify_retained_resident_assertion(record.get('old_owner_assertion'),
                                      'https://telemetry.example.com:7443', 'telemetry.example.com')
    for stage in ('fixture', 'restored', 'upgraded', 'fresh-candidate'):
        value = record.get('signals', {}).get(stage, {})
        require(value.get('raw_groups', 0) > 0 and len(value.get('verdicts', [])) >= 3
                and all(item.get('passed') is True for item in value['verdicts']),
                'restore/fresh candidate signal phase missing or invalid')
    require(record.get('native_older') == {'debian': True, 'fedora': True}
            and record.get('upgrade_preserved') is True, 'native same-code upgrade incomplete')
    for enrollment in record.get('owner_enrollments', []):
        flags = enrollment.get('flags', [])
        require(len(flags) == 2 and all(row.get('up') is True and row.get('uv') is True
                and row.get('rp_hash') == hashlib.sha256(b'telemetry.example.com').hexdigest() for row in flags),
                'actual owner UP/UV/RP evidence missing')
        require(len(enrollment.get('devices', [])) == 2
                and all(row.get('count') == 1 and row['credentials'] == [{'resident': True, 'rp': 'telemetry.example.com'}]
                        for row in enrollment['devices']), 'two independent resident devices required')
    require(len(record.get('owner_enrollments', [])) == 3, 'first/recovered/fresh owner enrollment required')
    require(record.get('cleanup_confirmed') is True, 'walkthrough cleanup unconfirmed')
    return {'exit': 0, 'classification': 'finite same-code native guide walkthrough; virtual authenticator only'}
