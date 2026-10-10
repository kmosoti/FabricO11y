#!/usr/bin/env python3
"""Preserve failed preflights and remove only owned campaign scratch."""
import json
import argparse
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT/'tools'))
from resource_group import require_limits,STORAGE
DATA=ROOT/'docs/experiments/benchmarks/data/query-plan-run-01'

def main():
    require_limits()
    parser=argparse.ArgumentParser()
    parser.add_argument('--report',default='cleanup-review.json',choices=['cleanup-review.json','cleanup-final.json'])
    args=parser.parse_args()
    copies=DATA/'coordinator/launcher-receipts';copies.mkdir(exist_ok=True)
    checks=json.loads((ROOT/'xtask/checks.json').read_text())['checks']
    verification=DATA/'recovery/fast-receipts';verification.mkdir(parents=True,exist_ok=True)
    for check in checks:
        if check['profile']=='fast':
            name=check['id']+'.json'
            shutil.copyfile(ROOT/'target/verification/receipts'/name,verification/name)
    records=[]
    for path in sorted((DATA/'coordinator').glob('*/receipt.json')):
        record=json.loads(path.read_text())
        if record['state']=='running':continue
        unit=record['cgroup'].rsplit('/',1)[-1].removesuffix('.service')
        source=ROOT/'target/resource-containment/runs'/(unit+'.json')
        shutil.copyfile(source,copies/(record['id']+'.json'))
        outer=json.loads(source.read_text())
        state=subprocess.run(['systemctl','--user','show',unit+'.service','--property=ActiveState','--value'],capture_output=True,text=True).stdout.strip()
        if state not in ('','inactive','failed'):raise RuntimeError('active completed unit')
        item={'id':record['id'],'exit':record['exit'],'service_state':state}
        failure=outer['retained_failure_evidence']
        if failure:
            retained=Path(failure)
            if retained.parent!=STORAGE/'evidence' or retained.name!=unit or retained.is_symlink():raise RuntimeError('ownership mismatch')
            if retained.exists():
                archive=DATA/'recovery'/(record['id']+'-failure.tar.gz');archive.parent.mkdir(exist_ok=True)
                compact = DATA/'memory/retry-02/full-failure/manifest.json'
                if record['id']=='allocation-retry-02' and not compact.is_file():
                    raise RuntimeError('full native failure input has not been preserved')
                if record['id']=='allocation-retry-02':
                    verified=json.loads((DATA/'recovery/profile-archive-verification.json').read_text())
                    if not verified['passed'] or verified['actual_wrapper_count']!=64:
                        raise RuntimeError('failure archive not independently verified')
                if not archive.exists():
                    with tarfile.open(archive,'x:gz') as tar:
                        for p in retained.rglob('*'):
                            if p.is_file() and not p.is_symlink() and 'bin' not in p.relative_to(retained).parts:
                                if record['id']=='allocation-retry-02' and 'full-plain' in p.relative_to(retained).parts:
                                    continue  # Exact ledgers/answers are in compact manifest.
                                tar.add(p,arcname=str(p.relative_to(retained)),recursive=False)
                if archive.stat().st_size>40*2**20:raise RuntimeError('failure evidence admission exceeded; retain scratch')
                item['removed_bytes']=sum(p.stat().st_size for p in retained.rglob('*') if p.is_file())
                item['preserved_failure']=str(archive.relative_to(DATA))
                shutil.rmtree(retained)
            item['scratch_removed']=not retained.exists()
        records.append(item)
    receipts=[json.loads(p.read_text()) for p in (DATA/'coordinator').glob('*/receipt.json')]
    completed=[r for r in receipts if r['state']!='running']
    usage={stage:sum(r['elapsed_s'] for r in completed if r['stage']==stage) for stage in ('native','profile','overhead')}
    for stage,limit in [('native',2400),('profile',900),('overhead',300)]:
        if usage[stage]>limit:raise RuntimeError('campaign stage budget exceeded')
    for r in completed:
        events=dict(line.split() for line in r['cgroup_final']['memory.events'].splitlines())
        if any(int(events[k]) for k in ('oom','oom_kill')) or int(r['cgroup_final']['memory.swap.current']):
            raise RuntimeError('OOM or swap observed')
    sizes={lab:sum(p.stat().st_size for p in (DATA/lab).rglob('*') if p.is_file()) for lab in ('coordinator','memory','query','recovery')}
    if any(n>50*2**20 for n in sizes.values()):raise RuntimeError('lab evidence cap exceeded')
    dest=DATA/args.report
    with dest.open('x') as stream:json.dump({'jobs':records,'shared_cache':'preserved','remote':'none',
        'completed_execution_seconds':usage,'lab_bytes':sizes,'oom_events':0,'swap_bytes':0,
        'limit':'Current cleanup job is still running; its own launcher writes final exit/cleanup after this report.'},stream,indent=2)
    print(json.dumps({'completed_jobs':len(records),'cleanup_report':str(dest)}))
if __name__=='__main__':main()
