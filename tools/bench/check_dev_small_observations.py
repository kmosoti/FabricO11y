#!/usr/bin/env python3
"""Cross-check archived profile accounting against raw events and samples."""
import argparse
import copy
import gzip
import json
import math
from collections import Counter
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from resource_group import require_limits


def read(path):
    return json.loads(path.read_text())


def lines(path):
    with gzip.open(path, 'rt') as stream:
        return [json.loads(s) for s in stream]


def checks(root, summary):
    problems = []
    def require(ok, reason):
        if not ok:
            problems.append(reason)
    sources = lines(root/'sources.jsonl.gz')
    sizes = {(r[0],r[1]):r[3] for r in lines(root/'recovered-hashes.jsonl.gz')}
    events = [(p.name.split('-')[0], r) for p in root.glob('node*-events.jsonl.gz') for r in lines(p)]
    cycles = {(node,int(r['batch'])):r for node,r in events if r['kind']=='cycle'}
    acks = {(node,int(r['sequence'])):r for node,r in events if r['kind']=='ack_attempt' and r['status']=='ack'}
    require(set(cycles)==set(acks)==set(sizes), 'cycle/ACK/recovery census differs')
    require(sum(int(r['logs']) for r in cycles.values())==len(sources)==summary['recovered_logs'], 'source/cycle/recovery log totals differ')
    buckets = read(root/'rates-per-second.json')
    total = Counter()
    for row in buckets:
        total.update({k:v for k,v in row.items() if k!='second'})
    for field, expected in [('spool_batches',len(cycles)), ('spool_bytes',sum(sizes.values())),
                            ('unique_acks',len(acks)), ('ack_bytes',sum(sizes.values())),
                            ('source_logs',len(sources)), ('source_bytes',len(sources)*901)]:
        require(total[field]==expected, 'bucket census '+field)
    epoch = summary['observation']['epoch_ns']
    for phase, offset in [('normal',0), ('burst',60), ('recovery',120)]:
        lo, hi = epoch+offset*10**9, epoch+(offset+60)*10**9
        expected = Counter()
        for node,r in events:
            if not lo <= r['t'] < hi:
                continue
            if r['kind']=='cycle':
                expected['spool_batches'] += 1
                expected['spool_bytes'] += sizes[(node,int(r['batch']))]
            else:
                expected['send_attempts'] += 1
                expected['status_'+r['status']] += 1
        expected['source_logs'] = sum(lo<=r[2]<hi for r in sources)
        actual = summary['observation']['rates'][phase]
        for key, value in expected.items():
            require(actual['counts'].get(key,0)==value, phase+' count '+key)
            require(math.isclose(actual['per_second'].get(key,0),value/60,abs_tol=1e-12), phase+' rate '+key)
    resources = read(root/'resources.json')
    cpu = summary['observation']['cpu']['whole']
    seconds = (resources[-1]['mono_ns']-resources[0]['mono_ns'])/10**9
    process_cpu = resources[-1]['processes'][0]['cpu_s']-resources[0]['processes'][0]['cpu_s']
    require(math.isclose(cpu['processes']['server']['mean_cores'],process_cpu/seconds,abs_tol=1e-12), 'server CPU mean')
    require(summary['observation']['cgroup_peak_bytes']==max(int(r['cgroup']['memory.peak']) for r in resources), 'cgroup peak')
    queries = lines(root/'queries.jsonl.gz')
    for shape, phases in summary['observation']['queries'].items():
        times = sorted(q['elapsed_ms'] for q in queries if q['shape']==shape)
        actual = phases['all']['latency_ms']
        require(actual['samples']==len(times), 'query sample count '+shape)
        require(actual['p99']==times[math.ceil(len(times)*.99)-1], 'query p99 '+shape)
    visibility = read(root/'visibility.json')
    require(len(visibility)==36, 'sentinel census')
    for target in visibility:
        if target['visible_ns'] is not None:
            key = (target['row']['node'], target['row']['sequence'])
            require(target['spool_observed_ns']==cycles[key]['t'], 'sentinel Spool join')
            require(target['ack_observed_ns']==acks[key]['t'], 'sentinel ACK join')
    return problems


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root', type=Path)
    args = parser.parse_args()
    require_limits()
    summary = read(args.root/'summary.json')
    actual = checks(args.root, summary)
    controls = {}
    for kind in ('rate', 'cpu', 'query'):
        changed = copy.deepcopy(summary)
        if kind=='rate':
            changed['observation']['rates']['burst']['per_second']['spool_batches'] += 1
        elif kind=='cpu':
            changed['observation']['cpu']['whole']['processes']['server']['mean_cores'] += 1
        else:
            changed['observation']['queries']['recent_logs']['all']['latency_ms']['p99'] += 1
        controls[kind] = checks(args.root, changed)
    report = {'problems':actual, 'negative_controls':controls,
              'passed':not actual and all(controls.values())}
    (args.root/'accounting-check.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report))
    raise SystemExit(0 if report['passed'] else 1)


if __name__=='__main__':
    main()
