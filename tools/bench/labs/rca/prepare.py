#!/usr/bin/env python3
"""Prepare bounded RCA producer fixtures; does not run or grade Fabric.

The fixture encoder is the existing independent Python test encoder. No oracle,
wire format, production behavior or expected checker outcome is modified.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
import resource_group
sys.path.insert(0, str(ROOT / 'tools/qualification'))
import test_query_oracle as encode

HERE = Path(__file__).resolve().parent
NS = 1_000_000_000


def sha(data):
    return hashlib.sha256(data).hexdigest()


def packets(deck):
    """Return producer inputs, query recipes and separate controller truth.

    No receive timestamp, ACK, query answer or measured diagnosis is invented.
    An execution driver must obtain those from Fabric and its own clocks.
    """
    epoch = deck['epoch_ns']
    cases = deck['cases']
    if len(cases) != 7 or len({c['id'] for c in cases}) != 7:
        raise ValueError('expected seven distinct frozen scenarios')
    producer, playbooks, truth = [], [], []
    for number, case in enumerate(cases, 1):
        scenario_id = case['id']
        if not scenario_id.replace('_', '').isalnum() or len(scenario_id) > 32:
            raise ValueError('invalid fixture case label')
        # Investigator-facing identifiers must not disclose a causal label.
        cid = f'rca-{number:02}'
        trace = hashlib.sha256(('rca-trace:' + cid).encode()).digest()[:16]
        api, downstream = cid + '-api', cid + '-dependency'
        span_ids = [i.to_bytes(8, 'big') for i in (1, 2, 3)]
        start = epoch + 10 * NS
        acquire = case['acquire_ms'] * 1_000_000
        duration = case['downstream_ms']
        attrs = {'fixture.case': cid, 'request.id': cid + '-request'}

        def gauge(name, value):
            point = encode.number_data_point(0, start, attrs, value, False)
            return encode.gauge_metric(name, 'By' if name.endswith('available') else '1', [point])

        def span(index, name, began, ended, parent):
            return encode.span(trace, span_ids[index], parent, name, 1,
                               began, ended, attrs, status_code=2 if index == 0 else 0)

        def emit(label, sequence, stage, metrics, logs, spans, gaps):
            identity = hashlib.sha256(('rca-node:' + label).encode()).digest()[:16]
            raw = encode.encode_batch(identity, 1, sequence,
                encode.metrics_request(metrics), encode.logs_request(logs), gaps,
                encode.traces_request(spans))
            if len(raw) >= 1024 * 1024:
                raise ValueError('fixture exceeds existing Batch envelope cap')
            producer.append(dict(case=cid, stage=stage, label=label,
                node_id=identity.hex(), generation=1, sequence=sequence,
                bytes=base64.b64encode(raw).decode('ascii'), sha256=sha(raw)))

        metrics = [gauge('app.pool.in_use', case['pool_in_use']),
                   gauge('app.pool.capacity', case['pool_capacity']),
                   gauge('system.memory.available', case['available_memory_bytes'])]
        points = case.get('counter_points', [
            dict(time_s=0, start_s=0, value=100),
            dict(time_s=10, start_s=0, value=160),
            dict(time_s=20, start_s=0, value=220)])
        metrics.append(encode.sum_metric('app.requests', '1', True, [
            encode.number_data_point(epoch + p['start_s'] * NS,
                epoch + p['time_s'] * NS, attrs, p['value'], False) for p in points]))
        logs = [encode.log_record(start, start,
                    f'case={cid} trace_id={trace.hex()} {body}', attrs)
                for body in case['logs']]
        emit(api, 1, 'initial', metrics, logs, [
            span(0, 'request', start, start + NS, b''),
            span(1, 'pool.acquire', start + 5_000_000,
                 start + 5_000_000 + acquire, span_ids[0])], case['gaps'])
        child = []
        if duration is not None:
            began = start + acquire + 10_000_000 + case['downstream_clock_offset_ms'] * 1_000_000
            child = [span(2, 'downstream.call', began,
                          began + duration * 1_000_000, span_ids[0])]
        # A fresh dependency metric deliberately does not imply fresh spans.
        emit(downstream, 1, 'initial',
             [gauge('system.memory.available', case['available_memory_bytes'])],
             [], [] if case['late_downstream'] else child, [])
        if case['late_downstream']:
            emit(downstream, 2, 'late', [], [], child, [])

        common = dict(from_ns=epoch - 5 * NS, to_ns=epoch + 60 * NS)
        queries = [dict(key='logs', query=dict(common, kind='logs', node=api,
                    contains='case=' + cid, limit=1)),
                   dict(key='coverage', query=dict(kind='logs', node=api,
                    from_ns=0, to_ns=9_000_000_000_000_000_000,
                    contains='case=' + cid, limit=1))]
        for key, name in [('pool_use', 'app.pool.in_use'),
                          ('pool_capacity', 'app.pool.capacity'),
                          ('memory', 'system.memory.available'),
                          ('counter', 'app.requests')]:
            queries.append(dict(key=key, query=dict(common, kind='metrics',
                node=api, name=name, limit=1)))
        queries += [dict(key='rate', query=dict(common, kind='rate', node=api,
                                              name='app.requests')),
                    dict(key='trace', query=dict(common, kind='spans',
                        trace_id=trace.hex(), limit=1))]
        playbooks.append(dict(case=cid, trace_id=trace.hex(), queries=queries,
            phases=['initial', 'after_late', 'after_publication', 'after_restart'],
            instructions=[
                'Read every page and retain each full answer envelope.',
                'Record snapshot, gaps, unavailable, retained window and freshness before drawing conclusions.',
                'Interpret complete as retained-scan completeness, not complete instrumentation.',
                'Hold the initial first page across staged delivery, then finish that old chain and run a fresh query.',
                'Use parent IDs and within-span duration; cross-host clocks may differ.',
                'Only this investigator directory may be provided to the investigator; controller files contain ground truth.']))
        truth.append(dict(case=cid, scenario_id=scenario_id,
            injected_condition=case['injected_condition'],
            expected_initial=case['expected_initial'],
            expected_after_late=case['expected_after_late'],
            expected_rates=case.get('expected_rates'),
            acquire_ms=case['acquire_ms'], downstream_ms=duration,
            remaining_unknown=case['remaining_unknown'],
            discriminator=case['discriminator'], negative_control=case['negative_control']))
    return producer, playbooks, truth


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--evidence', type=Path, required=True)
    args = parser.parse_args()
    resource_group.require_limits()
    temporary = Path(os.environ['TMPDIR']).resolve(strict=True)
    temporary.relative_to((resource_group.STORAGE / 'scratch').resolve(strict=True))
    out = temporary / 'rca-fixtures'
    evidence = args.evidence.resolve()
    evidence_root = ROOT / 'docs/experiments/benchmarks/data/hammer-reference-01/query'
    evidence.relative_to(evidence_root.resolve())
    if evidence == evidence_root.resolve() or evidence.exists():
        raise ValueError('use a fresh evidence child for this preparation')
    producer, playbooks, truth = packets(json.loads((HERE / 'scenarios.json').read_text()))
    documents = {
        'controller/producer.jsonl': ''.join(json.dumps(p) + '\n' for p in producer),
        'investigator/playbooks.json': json.dumps(playbooks, indent=2) + '\n',
        'controller/truth.json': json.dumps(truth, indent=2) + '\n',
        'controller/scenarios.json': (HERE / 'scenarios.json').read_text(),
        'controller/prepare.py': Path(__file__).read_text(),
    }
    if sum(len(value.encode()) for value in documents.values()) > 1024 * 1024:
        raise ValueError('prepared evidence exceeds 1 MiB bound')
    out.mkdir(mode=0o700, parents=False, exist_ok=False)
    for name, value in documents.items():
        (out / name).parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        (out / name).write_text(value)
    source_paths = [Path(__file__), HERE / 'scenarios.json',
                    Path(encode.__file__), ROOT / 'tools/qualification/query_oracle.py']
    (out / 'controller/manifest.json').write_text(json.dumps(dict(
        status='prepared_only_not_ingested', cases=len(truth), batches=len(producer),
        files={name:sha(value.encode()) for name,value in documents.items()},
        sources={str(p.relative_to(ROOT)):sha(p.read_bytes()) for p in source_paths},
        storage=str(out), retained_evidence=str(evidence), server_started=False, queries_executed=0,
        boundary='Synthetic producer payloads and recipes only; native outcomes remain unmeasured.'), indent=2) + '\n')
    evidence.mkdir(mode=0o700, parents=True, exist_ok=False)
    for path in out.rglob('*'):
        if not path.is_file():
            continue
        target = evidence / path.relative_to(out)
        target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        shutil.copyfile(path, target)
        if sha(path.read_bytes()) != sha(target.read_bytes()):
            raise RuntimeError('prepared evidence copy mismatch')
    print(evidence / 'controller/manifest.json')


if __name__ == '__main__':
    main()
