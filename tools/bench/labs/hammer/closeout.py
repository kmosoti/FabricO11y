"""Reconcile this pressure round without converting failed trials to passes."""
import copy
import json
import math
import os
from pathlib import Path
import shlex
import subprocess
import sys

import job
import service


def valid(record):
    elapsed = record.get('elapsed_s')
    return (record.get('state') in ('passed','failed')
            and isinstance(elapsed,(int,float)) and not isinstance(elapsed,bool)
            and math.isfinite(elapsed) and elapsed >= 0
            and isinstance(record.get('exit'),int)
            and ((record['state']=='passed') == (record['exit']==0))
            and record.get('limits',{}).get('memory.max')==str(20*2**30)
            and record.get('limits',{}).get('memory.swap.max')=='0')


def main():
    service.resource_group.require_limits()
    out = Path(sys.argv[1]); out.mkdir(parents=True,exist_ok=False)
    base = job.base.BASE
    own = Path(os.environ['FABRIC_SCRATCH_ROOT']).name
    receipts = {p.parent.name:(p,json.loads(p.read_text()))
                for p in (base/'coordinator').glob('*/receipt.json') if p.parent.name!=own}
    required = ('references-02','pressure-scan-02','identity-walk-01',
                'identity-scan-01','identity-scan-02','identity-walk-02','edge-real-regrade-01',
                'edge-sim-eight-01','network-01','remote-reduce-two-01',
                'remote-reduce-eight-01','identity-build-01','identity-library-01',
                'tail-identity-fixed-01','final-preflight-01','final-fast-01','final-docs-01')
    for name in required:
        if name not in receipts or receipts[name][1]['state']!='passed':
            raise RuntimeError('required completed case absent: '+name)
    preserved = {}
    for path in (base/'memory').glob('failure-preservation-*/summary.json'):
        for record in json.loads(path.read_text()):
            preserved[record['unit']] = record
    launchers = {}
    for path in (service.ROOT/'target/resource-containment/runs').glob('*.json'):
        record = json.loads(path.read_text()); command = record.get('command',[])
        if 'tools/bench/labs/hammer/job.py' in command and '--id' in command:
            name = command[command.index('--id')+1]
            if name in receipts: launchers[name] = (path,record)
    rows = {}
    for name,(path,record) in sorted(receipts.items()):
        if not valid(record) or name not in launchers:
            raise RuntimeError('invalid/incomplete receipt: '+name)
        launcher_path, launcher = launchers[name]
        if record.get('id') != name or record['argv'] != launcher['command'][launcher['command'].index('--')+1:]:
            raise RuntimeError('launcher/coordinator identity or command differs: '+name)
        if launcher['exit'] != record['exit']:
            raise RuntimeError('launcher/coordinator exits differ: '+name)
        scratch = Path(record['scratch'])
        scratch.relative_to(service.resource_group.STORAGE/'scratch')
        if scratch != Path(launcher['temporary'])/name:
            raise RuntimeError('launcher/coordinator scratch identity differs: '+name)
        if scratch.exists() or not launcher['temporary_removed']:
            raise RuntimeError('owned temporary tree remains: '+name)
        failed_tree = launcher.get('retained_failure_evidence')
        preservation = None
        if failed_tree:
            preservation = preserved.get(launcher['unit'])
            if (Path(failed_tree).exists() or preservation is None
                    or not preservation['removed']
                    or service.digest(Path(preservation['archive']))!=preservation['archive_sha256']):
                raise RuntimeError('failure lacks authenticated cleanup: '+name)
        active = subprocess.run(['systemctl','--user','is-active',launcher['unit']],capture_output=True,text=True)
        if active.stdout.strip() in ('active','activating','deactivating'):
            raise RuntimeError('owned local unit remains: '+name)
        rows[name] = dict(state=record['state'],exit=record['exit'],argv=record['argv'],
            elapsed_s=record['elapsed_s'],receipt_sha256=service.digest(path),
            launcher_sha256=service.digest(launcher_path),cgroup_final=record['cgroup_final'],
            original_scratch_absent=True,failure_preservation=preservation)
    good = next(r for _,r in receipts.values() if r['state']=='passed')
    rejected = []
    for key,value in [('state','running'),('exit',1),('elapsed_s',float('nan')),
                      ('elapsed_s',-1),('limits',{'memory.max':'max','memory.swap.max':'0'})]:
        bad = copy.deepcopy(good); bad[key] = value
        if valid(bad): raise RuntimeError('receipt defect admitted: '+key)
        rejected.append(key)
    service_rows = {}
    for name in ('identity-walk-01','identity-scan-01','identity-scan-02','identity-walk-02'):
        directory = base/'memory'/name
        summary = json.loads((directory/'summary.json').read_text())
        if not summary['passed'] or not all(summary['gates'].values()):
            raise RuntimeError('service semantic gate failed: '+name)
        samples = json.loads((directory/'resources.json').read_text())
        peaks = [int(r['child_cgroups']['server']['memory.peak']) for r in samples]
        service_rows[name] = {key:summary[key] for key in (
            'seed','recovered_logs','acknowledged_batches','server_peak_rss_mib',
            'server_cpu_seconds','encoded_batch_bytes','residency','service_objectives','native_attempt_rtt_ms')}
        service_rows[name].update(server_cgroup_peak_bytes=max(peaks),
            collection_to_ack_ms=summary['latency_ms']['all']['ack'],
            rates=summary['observation']['rates'],
            queries={key:value['all']['latency_ms'] for key,value in summary['observation']['queries'].items()},
            visibility=summary['observation']['visibility'],
            producer_lateness_ms=summary['observation']['producer_lateness_ms'])
    ledgers,_ = job.load()
    ledgers['native-frontier-01'] = [r for r in ledgers['native-frontier-01'] if r['id']!=own]
    usage = {lab:job.base.footprint(base/lab) for lab in job.base.CAPS}
    if any(usage[lab]>cap*2**20 for lab,cap in job.base.CAPS.items()):
        raise RuntimeError('evidence cap exceeded')
    inspection = """import json,pathlib,socket,subprocess
units=subprocess.run(['systemctl','list-units','--all','--plain','--no-legend','fabric-edge-*'],capture_output=True,text=True,check=True)
print(json.dumps({'host':socket.gethostname(),'scratch':[str(p) for p in pathlib.Path('/var/tmp').glob('fabric-hammer-*')],'units':units.stdout}))
"""
    remote = subprocess.run(['ssh','-o','BatchMode=yes','-o','ConnectTimeout=10',
        'digitalocean-02',shlex.join(['python3','-c',inspection])],capture_output=True,text=True,timeout=30,check=True)
    remote_state = json.loads(remote.stdout)
    if remote_state['host']!='digitalocean-02' or remote_state['scratch'] or remote_state['units'].strip():
        raise RuntimeError('remote cleanup inspection has remaining state: '+remote.stdout)
    result = dict(jobs=rows,receipt_controls_rejected=rejected,service=service_rows,
        cumulative_excluding_this_closeout=job.totals(ledgers),evidence_allocated_bytes=usage,
        remote_cleanup=remote_state,
        shared_build_cache=str(service.resource_group.STORAGE/'cargo'),shared_build_cache_removed=False,
        target_qualification=False,production_defaults_changed=False,
        boundary='Finite round completed; failed outcomes preserved. This closeout duration is charged by its enclosing receipt.')
    (out/'summary.json').write_text(json.dumps(result,indent=2)+'\n')
    (out/'closeout.py').write_bytes(Path(__file__).read_bytes())
    print(json.dumps(dict(jobs=len(rows),usage=usage,accounting=result['cumulative_excluding_this_closeout'])))


if __name__=='__main__': main()
