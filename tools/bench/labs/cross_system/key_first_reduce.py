#!/usr/bin/env python3
"""Independent bounded native key-first reduction; execute only through containment."""
import argparse
import copy
import gzip
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import statistics
import time

import query_census as c
import run_native_job as n

SHAPES = ('absent','selective','common','broad','empty-literal','unicode')


def require(condition, message):
    if not condition: raise RuntimeError(message)


def decision(rows, allocation, semantic):
    require(len(rows)==192,'192 distinct plain paired populations required')
    primary = [r for r in rows if (r['width'],r['attrs'],r['population']) ==
        (1024,8,'tail-scan-common-reuse')]
    require(len(primary)==1,'unique registered primary required')
    for r in rows:
        require(all(isinstance(r[k],(int,float)) and math.isfinite(r[k]) and r[k]>0
            for k in ('cpu_ratio','wall_ratio')),'finite positive ratios required')
    require({r['regime'] for r in allocation}=={'counted-on','counted-off'} and len(allocation)==2,
        'both counted primary populations required')
    primary_pass = primary[0]['cpu_ratio']<=.90 and primary[0]['wall_ratio']<=1.05
    allocation_pass = all(r['requested_ratio']<=1.05 and r['peak_ratio']<=1.05 for r in allocation)
    cpu = [r for r in rows if r['cpu_ratio']>1.05]
    wall = [r for r in rows if r['wall_ratio']>1.05]
    return dict(decision='confirm' if semantic and primary_pass and allocation_pass and not cpu and not wall else 'reject',
        primary=primary[0],primary_pass=primary_pass,allocation_pass=allocation_pass,
        failed_cpu_populations=len(cpu),failed_wall_populations=len(wall),
        failed_either_populations=len({(r['width'],r['attrs'],r['population']) for r in cpu+wall}),
        cpu_failures=cpu,wall_failures=wall)


def controls(rows, allocation):
    good = copy.deepcopy(rows)
    for row in good: row['cpu_ratio']=.89; row['wall_ratio']=1.0
    alloc = copy.deepcopy(allocation)
    for row in alloc: row['requested_ratio']=1.0; row['peak_ratio']=1.0
    require(decision(good,alloc,True)['decision']=='confirm','positive admission control rejected')
    rejected=[]
    for defect in ('primary_cpu','other_cpu','other_wall','allocation','semantic','missing_population','nonfinite'):
        bad=copy.deepcopy(good); a=copy.deepcopy(alloc); semantic=True
        if defect=='primary_cpu':
            next(r for r in bad if (r['width'],r['attrs'],r['population'])==(1024,8,'tail-scan-common-reuse'))['cpu_ratio']=.901
        elif defect=='other_cpu': bad[0]['cpu_ratio']=1.051
        elif defect=='other_wall': bad[0]['wall_ratio']=1.051
        elif defect=='allocation': a[0]['peak_ratio']=1.051
        elif defect=='semantic': semantic=False
        elif defect=='missing_population': bad.pop()
        else: bad[0]['cpu_ratio']=float('nan')
        try: rejected_flag=decision(bad,a,semantic)['decision']=='reject'
        except RuntimeError: rejected_flag=True
        require(rejected_flag,'reducer accepted injected '+defect)
        rejected.append(defect)
    return rejected


def main():
    c.profile.require_limits()
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--screen',type=Path,required=True)
    p.add_argument('--controls',type=Path,required=True)
    p.add_argument('--out',type=Path,required=True)
    p.add_argument('--seconds',type=int,default=300)
    a=p.parse_args()
    require(0<a.seconds<=600,'reduction deadline must be <=600 seconds')
    require(not a.out.exists(),'fresh reduction directory required')
    a.out.mkdir(parents=True)
    (a.out/'admission.json').write_text(json.dumps(dict(decision='reject',
        screen_result_sha256=c.grader.sha(a.screen/'result.json'),reason='independent validation pending; fail closed'),indent=2)+'\n')
    scratch=Path(os.environ['FABRIC_SCRATCH_ROOT'])/'key-first-reduction'
    scratch.mkdir()
    started=time.monotonic(); deadline=started+a.seconds
    hashes={}; verified={}
    def tick():
        require(time.monotonic()<deadline,'reduction deadline exceeded')
        require(n.footprint(n.BASE/'query')<384*1024**2,'query evidence category exceeded')
    def read(path):
        tick(); hashes[str(path)]=c.grader.sha(path)
        return json.loads(path.read_text())
    def payload(ref):
        path=a.screen/'objects'/ref
        require(path.parent==a.screen/'objects' and not path.is_symlink(),'unsafe object reference')
        with gzip.open(path,'rb') as stream: raw=stream.read(64*1024**2+1)
        require(len(raw)<=64*1024**2,'object exceeds64MiB bound')
        require(hashlib.sha256(raw).hexdigest()==ref[:64],'object decoded digest differs')
        verified[ref]=len(raw)
        return raw
    try:
        result=read(a.screen/'result.json'); trials=read(a.screen/'trials.json')
        require(result['trials']==trials and len(trials)==24,'trial summaries differ')
        require({(t['fixture']['width'],t['fixture']['attrs'],t['regime'],t['candidate']) for t in trials}==
            {(w,attrs,regime,flag) for w in (16,1024) for attrs in (0,8)
             for regime in ('counted-on','counted-off','plain') for flag in (False,True)},'matched child grid differs')
        control=read(a.controls/'result.json')
        require(control['native_test_commands']==8 and control['exit']==0 and control['flags']==[0,1],
            'native control receipt missing')
        require(len(control['exact_corruption_outcomes'])==2 and all(not v['complete'] and len(v['unavailable'])==1
            for v in control['exact_corruption_outcomes']),'native corrupt History outcome differs')
        by_trial={}; chains=pages=mutants=measures=0
        for trial in trials:
            tick(); folder=a.screen/trial['label']; native=read(folder/'native.out')
            require(native['fixture']==trial['fixture'],'native fixture differs')
            require(native['fixture']['key_first']==trial['candidate'] and not native['fixture']['borrowed']
                and native['fixture']['seed']==42 and native['fixture']['order']=='sorted','compiled flag/source differs')
            values=native['results']; require(len(values)==96,'96 measured calls required')
            coverage={(v['layout'],v['plan'],v['shape'],v['iteration']) for v in values}
            require(len(coverage)==96 and coverage=={(l,p,s,i) for l in ('tail','segment')
                for p in ('scan','walk') for s in SHAPES for i in range(4)},'measurement coverage differs')
            metrics={}
            for l in ('tail','segment'):
                for plan in ('scan','walk'):
                    for shape in SHAPES:
                        selected=sorted((v for v in values if (v['layout'],v['plan'],v['shape'])==(l,plan,shape)),key=lambda v:v['iteration'])
                        for boundary,subset in [('first',selected[:1]),('reuse',selected[1:])]:
                            metric={k:statistics.median(v[k] for v in subset) for k in ('cpu_ns','wall_ns')}
                            metric.update(requested=statistics.median(v['allocation']['total'] for v in subset) if native['fixture']['counted'] else None,
                                peak=statistics.median(v['allocation']['peak']-v['allocation']['base'] for v in subset) if native['fixture']['counted'] else None)
                            metrics[f'{l}-{plan}-{shape}-{boundary}']=metric
            require(metrics==trial['metrics'],'independent measured medians differ')
            by_trial[trial['label']]=metrics; measures+=96
            manifest=read(folder/'native-state-manifest.json')
            for entry in manifest.values():
                if entry['object'] not in verified:
                    raw=payload(entry['object'])
                    require(len(raw)==entry['bytes'] and hashlib.sha256(raw).hexdigest()==entry['sha256'],'native member differs')
                else: require(entry['object'][:64]==entry['sha256'] and verified[entry['object']]==entry['bytes'],
                    'native member digest/size map differs')
            record_path=scratch/'records.jsonl'
            record_path.write_bytes(payload(manifest['records.jsonl']['object']))
            records,_=c.grader.decode_records(record_path)
            require(c.grader.sha(record_path)==trial['source_sha256'],'exact producer source differs')
            maps=read(folder/'maps.json'); oracle=read(folder/'oracle.json')
            require(len(maps['chains'])==24 and len(oracle['verdicts'])==24 and len(oracle['rejected_controls'])==60,
                'chain/control coverage differs')
            for name,mapping in maps['chains'].items():
                answer=[json.loads(payload(ref)) for ref in mapping['pages']]
                verdict=c.grader.query_oracle.check(records,mapping['query'],answer)
                require(verdict['passed'],'independent full-chain regrade rejected '+name)
                require(all(v['passed'] for v in oracle['verdicts']),'original oracle rejection retained')
                chains+=1; pages+=len(answer)
            for mutant in oracle['rejected_controls']:
                bad=json.loads(payload(mutant['vector']))
                require(not mutant['verdict']['passed'] and not c.grader.query_oracle.check(records,mutant['query'],bad)['passed'],
                    'retained negative control accepted')
                mutants+=1
            record_path.unlink()
        require((chains,pages,mutants,measures)==(576,11040,1440,2304),'registered evidence counts differ')
        rows=[]; allocation=[]
        for trial in trials:
            if not trial['candidate']: continue
            f=trial['fixture']; before=next(t for t in trials if not t['candidate'] and t['regime']==trial['regime']
                and (t['fixture']['width'],t['fixture']['attrs'])==(f['width'],f['attrs']))
            for pop,metric in by_trial[trial['label']].items():
                old=by_trial[before['label']][pop]
                row=dict(width=f['width'],attrs=f['attrs'],population=pop,
                    baseline=old,candidate=metric,cpu_ratio=metric['cpu_ns']/old['cpu_ns'],wall_ratio=metric['wall_ns']/old['wall_ns'])
                if trial['regime']=='plain': rows.append(row)
                elif (f['width'],f['attrs'],pop)==(1024,8,'tail-scan-common-reuse'):
                    allocation.append(dict(regime=trial['regime'],requested_ratio=metric['requested']/old['requested'],
                        peak_ratio=metric['peak']/old['peak'],requested_saved_bytes=old['requested']-metric['requested'],baseline=old,candidate=metric))
        verdict=decision(rows,allocation,True)
        report=dict(**verdict,plain_population_count=len(rows),plain_rows=rows,primary_allocation=allocation,
            ratio_ranges={k:dict(min=min(rows,key=lambda r:r[k]),max=max(rows,key=lambda r:r[k])) for k in ('cpu_ratio','wall_ratio')},
            reducer_rejected_controls=controls(rows,allocation),evidence=dict(chains=chains,pages=pages,mutants=mutants,first_page_measurements=measures,
                decoded_objects_verified=len(verified),native_test_commands=8,control_outcomes=control['exact_corruption_outcomes']),input_sha256=hashes)
        for job in ('key-first-screen-01','key-first-controls-01'):
            receipt=read(n.BASE/'coordinator'/job/'receipt.json')
            require(receipt['exit']==0 and receipt['state']=='passed' and receipt['scratch_removed'],'coordinator job did not pass')
            report[job]=dict(elapsed_s=receipt['elapsed_s'],cgroup_final=receipt['cgroup_final'])
        (a.out/'report.json').write_text(json.dumps(report,indent=2)+'\n')
        (a.out/'admission.json').write_text(json.dumps(dict(decision=verdict['decision'],
            screen_result_sha256=c.grader.sha(a.screen/'result.json'),reason='all registered primary, allocation, semantic and 192 plain population guards',
            failed_cpu_populations=verdict['failed_cpu_populations'],failed_wall_populations=verdict['failed_wall_populations']),indent=2)+'\n')
        shutil.rmtree(scratch)
        print(json.dumps({k:report[k] for k in ('decision','plain_population_count','failed_cpu_populations','failed_wall_populations','failed_either_populations')}))
    except BaseException as error:
        (a.out/'failure.json').write_text(json.dumps(dict(error=repr(error),scratch=str(scratch)))+'\n')
        raise


if __name__=='__main__': main()
