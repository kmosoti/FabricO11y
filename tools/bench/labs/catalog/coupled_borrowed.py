"""C4 diagnostic only; separate from frozen CR2 criteria and harness."""
import coupled_admit
import argparse,gzip,hashlib,importlib.util,json,os,shutil,signal,statistics,subprocess,sys,time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[4]
sys.path.insert(0,str(ROOT/'tools'))
from resource_group import require_limits,STORAGE
spec=importlib.util.spec_from_file_location('c4_profile',ROOT/'tools/bench/labs/completion/profile.py');profile=importlib.util.module_from_spec(spec);spec.loader.exec_module(profile)
CAP=48*1024**2; FAILURE=16*1024**2

def dump(path,value):path.write_text(json.dumps(value,indent=2)+'\n')
def sha(path):return profile.grader.sha(path)
def size(root):
    files={}
    for p in root.rglob('*'):
        if p.is_file():
            s=p.stat();files[(s.st_dev,s.st_ino)]=(s.st_size,s.st_blocks*512)
    return max(sum(v[0] for v in files.values()),sum(v[1] for v in files.values()))
def bounds(work,out,reserve=0):
    coupled_admit.observe(reserve,reserve)
    if size(out)+reserve>CAP:raise RuntimeError('C4 projected/live48MiB evidence cap')
    if size(work)>8*1024**3 or shutil.disk_usage(STORAGE).free<16*1024**3:raise RuntimeError('scratch/reserve limit')
def run(argv,env,stdout,stderr,deadline,work,out):
    bounds(work,out,FAILURE)
    with stdout.open('wb') as so,stderr.open('wb') as se:
        child=subprocess.Popen(argv,cwd=ROOT,env=env,stdout=so,stderr=se,start_new_session=True)
        try:
            while child.poll() is None:
                bounds(work,out,FAILURE)
                if time.monotonic()>deadline:raise RuntimeError('C4 800s deadline')
                time.sleep(.25)
        finally:
            if child.poll() is None:os.killpg(child.pid,signal.SIGKILL);child.wait(timeout=10)
    if child.returncode:raise RuntimeError('child exit '+str(child.returncode))
def grade(work,out,pool):
    records,bodies=profile.grader.decode_records(work/'records.jsonl')
    if len(bodies)!=4096 or any(len(b)!=1024 for b in bodies):raise RuntimeError('fixture row/body drift')
    maps={};verdicts=[]
    for plan in ['walk','scan']:
        for shape in ['selective','broad']:
            q={'kind':'logs','from_ns':1600000000000000000,'to_ns':1600000000000004096,'limit':50}
            if shape=='selective':q['contains']='needle-C4'
            raws=(work/f'chain-{plan}-{shape}.jsonl').read_bytes().splitlines(keepends=True)
            pages=[json.loads(b) for b in raws]
            if len(pages)!=(1 if shape=='selective' else 82):raise RuntimeError('canonical drain coverage')
            verdict=profile.grader.query_oracle.check(records,q,pages)
            if not verdict['passed'] or verdict['expected_rows']!=(32 if shape=='selective' else 4096):raise RuntimeError('independent semantic oracle rejected')
            if shape=='broad':
                bad=json.loads(json.dumps(pages));bad[0]['rows'].append(bad[0]['rows'][0])
                if profile.grader.query_oracle.check(records,q,pages[:-1])['passed'] or profile.grader.query_oracle.check(records,q,bad)['passed']:raise RuntimeError('oracle negative accepted')
            calls=(work/f'calls-{plan}-{shape}.jsonl').read_bytes().splitlines(keepends=True)
            if len(calls)!=3 or any(b!=raws[0] for b in calls):raise RuntimeError('measured first-page association')
            refs=[profile.retain_bytes(b[:-1],pool,'.answer.json') for b in raws]
            maps[plan+'-'+shape]={'query':q,'pages':refs,'calls':[{'sha256':hashlib.sha256(b).hexdigest(),'first':refs[0]} for b in calls]};verdicts.append(verdict)
    ledger=profile.retain_ledger((work/'records.jsonl').read_bytes(),pool)
    dump(out/'maps.json',{'records':ledger,'chains':maps});dump(out/'oracle.json',{'verdicts':verdicts,'negative_controls_rejected':True})
    return sha(work/'records.jsonl'),{'canonical_chains':len(verdicts),'associated_first_pages':sum(len(chain['calls']) for chain in maps.values()),'canonical_pages':sum(len(chain['pages']) for chain in maps.values())}
def metrics(native):
    grouped={}
    for r in native['results']:grouped.setdefault(r['plan']+'-'+r['shape'],[]).append(r)
    if len(grouped)!=4 or any(len(v)!=3 for v in grouped.values()):raise RuntimeError('query count drift')
    return {k:{'cpu_ns':statistics.median(r['cpu_ns'] for r in v),'wall_ns':statistics.median(r['wall_ns'] for r in v),'requested':statistics.median(r['allocation']['total'] for r in v) if v[0]['allocation'] else None,'peak':statistics.median(r['allocation']['peak']-r['allocation']['base'] for r in v) if v[0]['allocation'] else None} for k,v in grouped.items()}
def main():
    require_limits();p=argparse.ArgumentParser();p.add_argument('--out',type=Path,required=True);args=p.parse_args();args.out.mkdir(parents=True,exist_ok=False)
    base=Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True)
    if not base.is_relative_to(STORAGE/'scratch'):raise RuntimeError('data-drive scratch required')
    work=base/'coupled-borrowed';work.mkdir();pool=args.out/'objects';pool.mkdir();archive=args.out/'binaries';archive.mkdir()
    deadline=time.monotonic()+800;env=dict(os.environ);env.pop('FABRIC_SPILL_WORKSPACE_EXPERIMENT',None);env.pop('BENCH_SHARED_CATALOG',None)
    binaries={};hashes={};trials=[];identities={};seen=set()
    try:
        controls=['cargo','test','--offline','--locked','-p','fabric-server','--lib','borrowed_log_tests']
        run(controls,dict(env,FABRIC_BORROWED_LOG_EXPERIMENT='1'),args.out/'controls.out',args.out/'controls.err',deadline,work,args.out)
        dump(args.out/'controls-command.json',{'argv':controls,'exit':0,'scope':'rejected-row attrs/type/identity validation'})
        for counted in [False,True]:
            for borrowed in [False,True]:
                name=('counted' if counted else 'plain')+('-borrowed' if borrowed else '-owned')
                argv=['cargo','build','--offline','--locked','--release','-p','fabric-server','--example','coupled_borrowed_probe']
                if counted:argv+=['--features','responsibility-alloc-probe,phase-probe']
                run(argv,dict(env,FABRIC_BORROWED_LOG_EXPERIMENT='1' if borrowed else '0'),args.out/(name+'.out'),args.out/(name+'.err'),deadline,work,args.out)
                binary=work/name;shutil.copy2(Path(os.environ['CARGO_TARGET_DIR'])/'release/examples/coupled_borrowed_probe',binary);binaries[name]=binary;hashes[name]=sha(binary)
                dump(args.out/(name+'-command.json'),{'argv':argv,'exit':0,'FABRIC_BORROWED_LOG_EXPERIMENT':'1' if borrowed else '0'})
                with binary.open('rb') as src,gzip.open(archive/(name+'.gz'),'wb') as dst:shutil.copyfileobj(src,dst)
                with gzip.open(archive/(name+'.gz'),'rb') as src:
                    if hashlib.file_digest(src,'sha256').hexdigest()!=hashes[name]:raise RuntimeError('archive binary readback mismatch')
                binary.unlink()
        sources=[Path(__file__),ROOT/'tools/bench/labs/catalog/coupled_admit.py',ROOT/'docs/experiments/benchmarks/coupled-launch-protocol.md',ROOT/'docs/experiments/benchmarks/coupled-borrowed-diagnostic-protocol.md',ROOT/'crates/fabric-server/examples/coupled_borrowed_probe.rs',ROOT/'crates/fabric-server/src/segment.rs',ROOT/'crates/fabric-server/src/query.rs',ROOT/'crates/fabric-server/src/rows.rs',ROOT/'tools/qualification/query_oracle.py',ROOT/'tools/bench/labs/completion/profile.py',ROOT/'docs/experiments/benchmarks/coupled-capacity-preparation.md',ROOT/'docs/experiments/benchmarks/coupled-launch-protocol.md',ROOT/'tools/bench/labs/catalog/coupled_admit.py',ROOT/'tools/bench/labs/catalog/campaign_summary.py',ROOT/'tools/bench/labs/completion/run_job.py',ROOT/'tools/resource_group.py']
        dump(args.out/'provenance.json',{'revision':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),'binary_sha256':hashes,'source_sha256':{str(p.relative_to(ROOT)):sha(p) for p in sources}})
        for regime in ['plain','counted-off','counted-on']:
            for attrs in [0,8]:
                # One diagnostic pair; reverse rich order to reduce a fixed order bias.
                for borrowed in ([False,True] if attrs==0 else [True,False]):
                    bounds(work,args.out,FAILURE+4*1024**2*(attrs not in seen)+1024**2)
                    name=('plain' if regime=='plain' else 'counted')+('-borrowed' if borrowed else '-owned');stem=f'{regime}-attrs{attrs}-'+('borrowed' if borrowed else 'owned');evidence=args.out/stem;evidence.mkdir();trial=work/stem
                    binary=binaries[name]
                    with gzip.open(archive/(name+'.gz'),'rb') as src,binary.open('wb') as dst:shutil.copyfileobj(src,dst)
                    binary.chmod(0o700)
                    if sha(binary)!=hashes[name]:raise RuntimeError('restored binary hash mismatch')
                    argv=[str(binary),str(trial),str(attrs)];native_env=dict(env,BENCH_PHASES='1' if regime=='counted-on' else '0')
                    run(argv,native_env,evidence/'timings.json',evidence/'probe.err',deadline,work,args.out)
                    native=json.loads((evidence/'timings.json').read_text());f=native['fixture']
                    if any(f[k]!=v for k,v in {'rows':4096,'attrs':attrs,'body_bytes':1024,'seed':42,'borrowed':borrowed,'phase_on':regime=='counted-on','counted':regime!='plain','limit':50}.items()):raise RuntimeError('fixture/mode acknowledgement')
                    identity,structural=grade(trial,evidence,pool)
                    if attrs in identities and identities[attrs]!=identity:raise RuntimeError('matched source drift')
                    identities[attrs]=identity;seen.add(attrs)
                    phases={}
                    for path in trial.glob('*phases.jsonl'):profile.grader.compress(path,evidence/(path.name+'.gz'))
                    for path in trial.glob('measured-*.jsonl'):
                        rows=[json.loads(l) for l in path.read_text().splitlines()]
                        if len(rows)>8192:raise RuntimeError('phase record limit')
                        phases[path.name]=rows;profile.grader.compress(path,evidence/(path.name+'.gz'))
                    for pattern in ['warmup-*.jsonl','continuation-*.jsonl']:
                        for path in trial.glob(pattern):profile.grader.compress(path,evidence/(path.name+'.gz'))
                    if regime=='counted-on' and len(phases)!=12:raise RuntimeError('measured phase coverage')
                    if regime!='counted-on' and phases:raise RuntimeError('observer-off emitted spans')
                    dump(evidence/'command.json',{'argv':argv,'exit':0,'binary_sha256':hashes[name],'BENCH_PHASES':native_env['BENCH_PHASES']})
                    trials.append({'regime':regime,'attrs':attrs,'borrowed':borrowed,'fixture':f,'metrics':metrics(native),'phases':phases,'structural':structural});dump(args.out/'trials.json',trials)
                    if sha(binary)!=hashes[name]:raise RuntimeError('active binary changed')
                    binary.unlink();shutil.rmtree(trial);bounds(work,args.out,FAILURE)
        observer={}
        for attrs in [0,8]:
            for borrowed in [False,True]:
                off=next(t for t in trials if t['regime']=='counted-off' and t['attrs']==attrs and t['borrowed']==borrowed);on=next(t for t in trials if t['regime']=='counted-on' and t['attrs']==attrs and t['borrowed']==borrowed)
                for key in off['metrics']:
                    observer[f'{attrs}-{borrowed}-{key}']={m:on['metrics'][key][m]/off['metrics'][key][m] for m in ['cpu_ns','wall_ns','requested']}
        attribution=[];invalid_residuals=[]
        for trial in trials:
            if trial['regime']!='counted-on':continue
            for name,rows in trial['phases'].items():
                totals=[r for r in rows if r['phase']=='query_run_inclusive']
                if len(totals)!=1:raise RuntimeError('inclusive query phase association')
                total=totals[0];children=[r for r in rows if r['depth']==total['depth']+1]
                exclusive=sum(r['thread_cpu_ns'] for r in children)
                residual=total['thread_cpu_ns']-exclusive
                if residual < -max(1000,total['thread_cpu_ns']*.01):invalid_residuals.append({'attrs':trial['attrs'],'borrowed':trial['borrowed'],'file':name,'residual':residual})
                attribution.append({'attrs':trial['attrs'],'borrowed':trial['borrowed'],'file':name,'total_cpu_ns':total['thread_cpu_ns'],'direct_child_cpu_ns':exclusive,'unattributed_cpu_ns':residual,'negative_tolerance_ns':max(1000,total['thread_cpu_ns']*.01)})
        dump(args.out/'result.json',{'invalid_residuals':invalid_residuals,'attribution_reconciled':not invalid_residuals,'attribution':attribution,'trials':trials,'observer_ratios':observer,'observer_admitted':all(.95<=r[m]<=1.05 for r in observer.values() for m in r),'canonical_chains':sum(t['structural']['canonical_chains'] for t in trials),'associated_first_pages':sum(t['structural']['associated_first_pages'] for t in trials),'diagnostic_only':True,'isolated_attribute_parse_not_measured':True})
        shutil.rmtree(work);dump(args.out/'cleanup.json',{'removed':True})
    except BaseException as error:
        # Remove only exact duplicates of already readback-verified persistent archives.
        removed=[]
        for name,binary in binaries.items():
            if binary.exists() and name in hashes and sha(binary)==hashes[name]:
                with gzip.open(archive/(name+'.gz'),'rb') as src:
                    if hashlib.file_digest(src,'sha256').hexdigest()==hashes[name]:binary.unlink();removed.append(name)
        dump(args.out/'failure.json',{'error':repr(error),'retained_scratch':str(work),'verified_duplicate_binaries_removed':removed});raise
if __name__=='__main__':main()
