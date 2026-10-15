"""Independent raw-journal/query joins for the registered owner walkthrough.

No application codec is imported. Existing snapshot and query oracles retain
all of their framing, row, page, completeness and freshness checks.
"""
import base64
import hashlib

import query_oracle
from cross_family.snapshot import decode, select


def require(value, reason):
    if not value:
        raise ValueError(reason)


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
    for query, pages in queries:
        records = select(groups, pages)
        verdict = query_oracle.check(records, query, pages)
        require(verdict['passed'], 'independent query oracle rejected walkthrough answer')
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
