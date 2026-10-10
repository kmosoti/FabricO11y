#!/usr/bin/env python3
"""Independent partial-projection checker; producer-derived projection and raw availability.

Producer records, query and pages retain query_oracle's formats. Availability is
{raw_available:true, missing_projections:[{projection:logs|metrics|spans,
 node:str,sequence:int} OR {projection:...,from_ns:int,to_ns:int}]}.
This companion preserves all raw-derived envelope facts while excluding only
rows of an unavailable requested projection. raw_available:false declares loss of all raw sources in this fixture while
verified projections/Manifest/gaps survive. It cannot grade Manifest/gap loss,
filesystem integrity, token authenticity or an HTTP Gone response.
"""
import argparse
import base64
import json
import query_oracle as qo


def selectors(availability):
    if not isinstance(availability, dict) or set(availability) != {'raw_available', 'missing_projections'}:
        raise qo.MalformedInput('availability must contain exactly raw_available and missing_projections')
    if not isinstance(availability['raw_available'], bool):
        raise qo.MalformedInput('raw_available must be boolean')
    items = availability['missing_projections']
    if not isinstance(items, list):
        raise qo.MalformedInput('missing_projections must be a list')
    grouped, seen = {}, set()
    for item in items:
        if not isinstance(item, dict) or item.get('projection') not in ('logs', 'metrics', 'spans'):
            raise qo.MalformedInput('invalid projection selector')
        selector = {k: v for k, v in item.items() if k != 'projection'}
        specs = qo.validate_unavailable([selector])
        if 'sequence' in selector and selector['sequence'] < 0:
            raise qo.MalformedInput('sequence must be nonnegative')
        if 'from_ns' in selector and not (0 <= selector['from_ns'] < selector['to_ns'] <= 2**64 - 1):
            raise qo.MalformedInput('invalid projection receive interval')
        identity = json.dumps(item, sort_keys=True)
        if identity in seen:
            raise qo.MalformedInput('duplicate projection selector')
        seen.add(identity)
        grouped.setdefault(item['projection'], []).extend(specs)
    return grouped


def expected(records, query, availability):
    grouped = selectors(availability)
    full = qo.expected(records, query)  # All envelope facts come from producer bytes.
    mats = qo.materialize_all(records)
    batches = [qo.decode_batch(base64.b64decode(record['bytes'], validate=True)) for record in records]
    identities = [(batch['node_id'], batch['generation'], batch['sequence']) for batch in batches]
    if len(set(identities)) != len(identities):
        raise qo.MalformedInput('duplicate producer record identity')
    projection = 'metrics' if query['kind'] == 'rate' else query['kind']
    specs = grouped.get(projection, [])
    retained = [r for r, m in zip(records, mats) if not qo._is_unavailable(m, specs)]
    excluded = [m for m in mats if qo._is_unavailable(m, specs)]
    full['rows'] = qo.expected(retained, query)['rows']
    if not availability['raw_available'] and not any(qo._record_could_match(m, query) for m in mats):
        raise qo.MalformedInput('raw-loss companion scope requires a nonempty matching producer population')
    unknown = excluded if availability['raw_available'] else mats
    complete = not any(qo._record_could_match(m, query) for m in unknown)
    full['complete'] = complete
    full['unavailable_nonempty'] = not complete
    return full


def check(records, query, pages, availability):
    try:
        exp = expected(records, query, availability)
        qo._validate_pages_shape(pages, query)
    except qo.MalformedInput as error:
        return {'passed': False, 'violations': [{'rule': 'MALFORMED', 'detail': str(error)}],
                'expected_rows': None, 'answered_rows': None}
    violations = (qo._check_rate(exp, pages, query) if query['kind'] == 'rate'
                  else qo._check_paginated(exp, pages, query))
    violations.extend(qo._check_envelope(exp, pages, query))
    return {'passed': not violations,
            'violations': [{'rule': rule, 'detail': detail} for rule, detail in violations],
            'expected_rows': len(exp['rows']), 'answered_rows': sum(len(p['rows']) for p in pages)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('records', 'query', 'answer', 'availability'):
        parser.add_argument('--' + name, required=True)
    args = parser.parse_args()
    try:
        verdict = check(qo.load_records_jsonl(args.records), qo.load_json_file(args.query),
                        qo.load_json_file(args.answer), qo.load_json_file(args.availability))
    except (qo.MalformedInput, OSError) as error:
        verdict = {'passed': False, 'violations': [{'rule': 'MALFORMED', 'detail': str(error)}],
                   'expected_rows': None, 'answered_rows': None}
    print(json.dumps(verdict))
    if any(v['rule'] == 'MALFORMED' for v in verdict['violations']):
        return 2
    return 0 if verdict['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
