"""Preserve final checks and audit only this completion's owned run artifacts."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import coupled_admit
import coupled_resource_readiness
from resource_group import require_limits
ROOT=Path(__file__).resolve().parents[4]
DATA=ROOT/'docs/experiments/benchmarks/data'
COORD=DATA/'lab-completion-run-01/coordinator'
ID='catalog-coupled-completion-closeout-01'
def main():
    require_limits()
    out=DATA/ID;out.mkdir(exist_ok=False)
    receipts=out/'fast-receipts';receipts.mkdir()
    registered=json.loads((ROOT/'xtask/checks.json').read_text())['checks']
    manifest={}
    for check in registered:
        if check['profile']!='fast':continue
        source=ROOT/'target/verification/receipts'/(check['id']+'.json')
        raw=source.read_bytes();target=receipts/source.name;target.write_bytes(raw)
        if target.read_bytes()!=raw:raise RuntimeError('fast receipt copy drift')
        manifest[source.name]={'sha256':hashlib.sha256(raw).hexdigest(),'exit':json.loads(raw)['exit_code']}
    (out/'fast-receipts.json').write_text(json.dumps(manifest,indent=2)+'\n')
    command=[sys.executable,'-B','tools/bench/labs/completion/checks.py','--profile','documentation','--id',ID]
    print(json.dumps({'command':command}),flush=True)
    docs=subprocess.run(command).returncode
    start=json.loads((COORD/'catalog-coupled-dispatch-audit-01/receipt.json').read_text())['started_unix_ns']
    runs=[];copies=COORD/'launcher-receipts';copies.mkdir(exist_ok=True)
    for path in COORD.glob('*/receipt.json'):
        row=json.loads(path.read_text())
        if row['started_unix_ns']<start or row['id']==ID:continue
        if row['state']=='running':raise RuntimeError('prior run still active')
        unit=row['cgroup'].rsplit('/',1)[-1].removesuffix('.service')
        original=ROOT/'target/resource-containment/runs'/(unit+'.json')
        outer=json.loads(original.read_text())
        raw=original.read_bytes();copy=copies/(row['id']+'.json')
        if copy.exists() and copy.read_bytes()!=raw:raise RuntimeError('outer receipt changed')
        copy.write_bytes(raw)
        if copy.read_bytes()!=raw:raise RuntimeError('outer receipt copy differs')
        retained=outer.get('retained_failure_evidence')
        if retained and Path(retained).exists():raise RuntimeError('owned failed scratch still retained: '+row['id'])
        runs.append({k:row.get(k) for k in ('id','state','exit','elapsed_s','terminated_descendants')})
    coupled_resource_readiness.controls()
    result={'documentation_exit':docs,'fast_exits':manifest,'completion_jobs':runs,
        'prior_completion_job_count':len(runs),'owned_failed_scratch_absent':True,
        'command_outcomes_are_not_scientific_acceptance':True,
        'admission':coupled_admit.observe(0,0),'historical_failures_unchanged':True,
        'limitation':'current coordinator and outer receipt complete after this inventory'}
    (out/'result.json').write_text(json.dumps(result,indent=2)+'\n')
    result['admission']=coupled_admit.observe(0,0)
    (out/'result.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result),flush=True)
    if docs or any(v['exit'] for v in manifest.values()):raise SystemExit(1)
if __name__=='__main__':main()
