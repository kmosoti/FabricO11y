"""Reconcile this finite round's actual commands, containment and cleanup."""
import copy
import json
import math
import os
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0,str(ROOT/'tools/bench/labs/cross_system'))
import run_native_job as ledger

JOBS = ['service-build-01','service-controls-01','readiness-scan-01','readiness-walk-01',
        'recovery-controls-01','recovery-faults-01','service-evidence-01','service-results-01',
        'service-controls-02','service-fast-01','service-docs-01']


def acceptable(record, name):
    return (record.get('id')==name and record.get('state')=='passed'
            and record.get('exit')==0 and record.get('child_exit')==0
            and isinstance(record.get('elapsed_s'),(int,float))
            and not isinstance(record['elapsed_s'],bool)
            and math.isfinite(record['elapsed_s']) and record['elapsed_s'] >= 0
            and record.get('scratch_removed') is True
            and record.get('limits',{}).get('memory.max')==str(20*1024**3)
            and record.get('limits',{}).get('memory.swap.max')=='0')


def main():
    ledger.require_limits()
    out = ledger.BASE/'memory/service-recovery/closeout'
    out.mkdir(exist_ok=False)
    records = {}
    for name in JOBS:
        path = ledger.BASE/'coordinator'/name/'receipt.json'
        record = json.loads(path.read_text())
        if not acceptable(record,name):
            raise RuntimeError('unclosed or unsafe job: '+name)
        scratch = Path(record['scratch'])
        scratch.relative_to(ledger.STORAGE/'scratch')
        if scratch.exists():
            raise RuntimeError('owned scratch remains: '+name)
        records[name] = dict(argv=record['argv'],exit=record['exit'],elapsed_s=record['elapsed_s'],
            receipt_sha256=ledger.sha(path),cgroup_final=record['cgroup_final'],
            scratch_removed=True)
    good = json.loads((ledger.BASE/'coordinator'/JOBS[0]/'receipt.json').read_text())
    rejected = []
    for key,value in [('id','wrong'),('state','failed'),('exit',1),('child_exit',1),
                      ('elapsed_s',float('nan')),('scratch_removed',False),
                      ('limits',{'memory.max':'max','memory.swap.max':'0'})]:
        broken = copy.deepcopy(good)
        broken[key] = value
        if acceptable(broken,JOBS[0]):
            raise RuntimeError('receipt defect accepted: '+key)
        rejected.append(key)
    all_ledgers,_ = ledger.load_ledgers()
    own = Path(os.environ['FABRIC_SCRATCH_ROOT']).name
    all_ledgers['native-frontier-01'] = [r for r in all_ledgers['native-frontier-01'] if r['id']!=own]
    usage = {lab:ledger.footprint(ledger.BASE/lab) for lab in ledger.CAPS}
    post_start = {}
    for plan in ('scan','walk'):
        directory = ledger.BASE/'memory'/f'readiness-{plan}-01'
        summary = json.loads((directory/'summary.json').read_text())
        samples = json.loads((directory/'resources.json').read_text())
        epoch = summary['observation']['epoch_ns']
        rows = [r for r in samples if r['wall_ns']>=epoch and all(p['batch']>0 for p in r['progress'])]
        post_start[plan] = dict(samples=len(rows),
            max_node_sampled_rss_mib=max(p['rss_kib'] for r in rows for p in r['nodes'])/1024,
            max_sum_node_sampled_rss_mib=max(sum(p['rss_kib'] for p in r['nodes']) for r in rows)/1024,
            boundary='Post hoc: at/after offer origin and all nodes have emitted a first native Batch; not a new gate')
    result = dict(state='complete',jobs=records,
        this_round_prior_elapsed_s=sum(r['elapsed_s'] for r in records.values()),
        cumulative_excluding_this_closeout=ledger.ledger_totals(all_ledgers),
        imported_goal_edit_seconds=20.063403844833374,
        evidence_allocated_bytes=usage,
        category_cap_bytes={lab:cap*2**20 for lab,cap in ledger.CAPS.items()},
        rejected_receipt_controls=rejected,
        post_startup_collector_observations=post_start,
        host=dict(uname=list(os.uname()),os_release=Path('/etc/os-release').read_text(),
                  affinity=sorted(os.sched_getaffinity(0)),
                  cpu_model=next((line.split(':',1)[1].strip() for line in Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')),None)),
        shared_build_cache=str(ledger.STORAGE/'cargo'),shared_build_cache_removed=False,
        new_production_mechanism=False,target_qualification=False,
        limit='Own completed duration and final documentation reconciliation are charged by subsequent coordinator receipts.')
    ledger.dump(out/'summary.json',result)
    print(json.dumps({k:result[k] for k in ('state','this_round_prior_elapsed_s','cumulative_excluding_this_closeout','evidence_allocated_bytes')},indent=2))


if __name__=='__main__':
    main()
