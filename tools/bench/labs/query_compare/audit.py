"""Add independently recounted demand/plan provenance to the custody audit."""
import argparse
from collections import Counter
import copy
import gzip
import hashlib
import importlib.util
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[4]
spec = importlib.util.spec_from_file_location('custody_audit', Path(__file__).parent.parent/'dev_small/audit.py')
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)


def demand(rows, targets, epoch, declared, actual):
    errors = []
    ordinary = sorted((r for r in rows if r['shape'] != 'visibility'), key=lambda r:r['scheduled_ns'])
    visibility = sorted((r for r in rows if r['shape'] == 'visibility'), key=lambda r:r['scheduled_ns'])
    expected = [(epoch + tick*10**9, ('recent_logs','absent_text','cpu_metrics')[tick%3]) for tick in range(200)]
    if [(r['scheduled_ns'],r['shape']) for r in ordinary] != expected:
        errors.append('ordinary schedule/shape/count mismatch')
    if len(targets)!=36 or [r['scheduled_ns'] for r in visibility] != sorted(t['source_ns']+3*10**9 for t in targets):
        errors.append('visibility schedule/count mismatch')
    starts = Counter((int((r['start_ns']-epoch)//(60*10**9)), r['shape']) for r in ordinary)
    if any(starts[(phase,shape)]!=20 for phase in range(3) for shape in ('recent_logs','absent_text','cpu_metrics')):
        errors.append('started phase population mismatch')
    late = [r['start_ns']-r['scheduled_ns'] for r in rows]
    if not late or min(late)<0 or max(late)>100_000_000:
        errors.append('request starts late/early beyond demand guard')
    events = sorted([(r['start_ns'],1) for r in rows]+[(r['end_ns'],-1) for r in rows])
    current = peak = 0
    for _, delta in events:
        current += delta
        peak = max(peak,current)
    if peak>2 or current or any(r['end_ns']<r['start_ns'] for r in rows):
        errors.append('invalid concurrency/duration')
    if declared not in ('scan','walk') or declared!=actual:
        errors.append('query plan mismatch')
    return {'problems':errors, 'ordinary_count':len(ordinary), 'visibility_count':len(visibility),
            'max_lateness_ns':max(late,default=None),'max_concurrency':peak,
            'started_phase_shape_counts':{f'{p}:{s}':n for (p,s),n in sorted(starts.items())},
            'passed':not errors}


def controls():
    epoch = 10**18
    rows = [{'scheduled_ns':epoch+i*10**9, 'start_ns':epoch+i*10**9+10**6,
             'end_ns':epoch+i*10**9+20*10**6,
             'shape':('recent_logs','absent_text','cpu_metrics')[i%3]} for i in range(200)]
    targets = [{'source_ns':epoch+i*5*10**9} for i in range(36)]
    rows += [{'scheduled_ns':t['source_ns']+3*10**9,
              'start_ns':t['source_ns']+3*10**9+10**6,
              'end_ns':t['source_ns']+3*10**9+20*10**6, 'shape':'visibility'} for t in targets]
    assert demand(rows,targets,epoch,'scan','scan')['passed']
    assert not demand(rows[1:],targets,epoch,'scan','scan')['passed']
    delayed=copy.deepcopy(rows); delayed[0]['start_ns']+=200_000_000; delayed[0]['end_ns']+=200_000_000
    assert not demand(delayed,targets,epoch,'scan','scan')['passed']
    assert not demand(rows,targets,epoch,'scan','walk')['passed']
    duplicate=rows+[rows[0]]
    assert not demand(duplicate,targets,epoch,'scan','scan')['passed']
    return {'baseline_accepted':True,'missing_duplicate_late_plan_defects_rejected':True}


def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--cell',type=Path,required=True); args=ap.parse_args()
    base.require_limits()
    root=args.cell
    result=base.audit_data(root)
    env=json.loads((root/'environment.json').read_text())
    summary=json.loads((root/'summary.json').read_text())
    config=json.loads((root/'query-config.json').read_text())
    with gzip.open(root/'queries.jsonl.gz','rt') as stream:
        rows=[json.loads(line) for line in stream]
    targets=json.loads((root/'visibility.json').read_text())
    result['query_workload']=demand(rows,targets,summary['observation']['epoch_ns'],env['query_plan'],config['query_plan'])
    campaign=env['campaign_protocol']
    expected='docs/experiments/benchmarks/query-plan-protocol.md'
    if campaign['path']!=expected or base.digest(ROOT/expected)!=campaign['sha256']:
        result['problems'].append('campaign protocol mismatch')
    if base.digest(root/'server.conf')!=config['config_sha256']:
        result['problems'].append('query configuration hash mismatch')
    result['problems'].extend(result['query_workload']['problems'])
    result['passed']=not result['problems']
    with (root/'independent-audit.json').open('x') as stream:
        json.dump(result,stream,indent=2); stream.write('\n')
    print(json.dumps(result)); raise SystemExit(0 if result['passed'] else 1)


if __name__=='__main__':
    main()
