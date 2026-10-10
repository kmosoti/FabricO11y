#!/usr/bin/env python3
"""Sequential Fedora extension; unchanged independent query oracle."""
import argparse,base64,gzip,hashlib,json,os,shutil,statistics,subprocess,sys,time
from collections import Counter,defaultdict
from pathlib import Path
REPO=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(REPO/'tools/bench'))
from run_responsibility_isolation import dump,compress,decode_records,exact,grade_query,negative_controls,quantiles,footprint
from run_dev_small import process_stats

def memory_available():
    return int(next(x.split()[1] for x in Path('/proc/meminfo').read_text().splitlines() if x.startswith('MemAvailable:')))*1024

def run(root,bins,case,repeat,variant,preflight=False):
    print(f"start {case['id']} r{repeat} {variant}",flush=True)
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT'])
    label=f"{case['id']}-r{repeat}-{variant}";dest=root/'evidence'/label;work=scratch/'performance-refinement'/label
    assert not dest.exists() and not work.exists(),label
    dest.mkdir(parents=True);work.mkdir(parents=True);(work/'owned').write_text(label)
    assert memory_available() >=12*2**30,'RAM headroom'
    assert shutil.disk_usage(work).free>=20*2**30,'disk headroom'
    env=dict(os.environ,BENCH_RECORDS=str(case.get('records',4096)),BENCH_BODY_SIZE='1024',BENCH_ORDER='shuffled',BENCH_QUERY_REPEATS='3',BENCH_QUERY_ROTATION=str((repeat-1)%4),BENCH_PREFLIGHT='1' if preflight else '0',BENCH_PHASES='0' if variant=='plain' else '1',BENCH_MEMORY_GUARD_BYTES=str(8*2**30),BENCH_ROTATE_BYTES=str(case.get('rotate',64*2**20)),BENCH_BAD_SOURCES=str(case.get('bad',0)))
    argv=['prlimit',f'--as={8*2**30}','--',str(bins/variant),str(work),case['mode'],str(case.get('workers',1024))]
    dump(dest/'command.json',{'argv':argv,'env':{k:v for k,v in env.items() if k.startswith('BENCH_')},'case':case,'repeat':repeat,'variant':variant,'preflight':preflight,'source_revision':subprocess.check_output(['git','rev-parse','HEAD'],cwd=REPO,text=True).strip(),'binary_sha256':hashlib.sha256((bins/variant).read_bytes()).hexdigest(),'available_memory_before':memory_available()})
    p=None;mon=[];began=time.monotonic();last_sample=0
    try:
        with (work/'timings.jsonl').open('wb') as out,(dest/'stderr.txt').open('wb') as err:
            p=subprocess.Popen(argv,env=env,stdout=out,stderr=err)
            while p.poll() is None:
                now=time.monotonic()
                if now-began>300:raise RuntimeError('300s child bound')
                if now-last_sample>=.5:
                    try:mon.append({'mono_ns':time.monotonic_ns(),'process':process_stats(p.pid),'io':Path(f'/proc/{p.pid}/io').read_text(),'memory_available':memory_available()})
                    except FileNotFoundError:pass
                    last_sample=now
                time.sleep(.05)
        assert p.returncode==0,f'probe exit {p.returncode}'
        rows=[json.loads(x) for x in (work/'timings.jsonl').read_text().splitlines()]
        assert rows[-1]['stage']=='complete'
        expected=(work/'source.log').read_text().splitlines()
        # Assembly gap cases intentionally preserve visible coverage notices.
        if case['mode']=='assembly':
            import query_oracle
            bodies=[]
            for line in (work/'records.jsonl').read_text().splitlines():
                r=json.loads(line);batch=query_oracle.decode_batch(bytes.fromhex(r['hex']))
                bodies.extend(x['body'] for x in query_oracle.decode_logs_request(batch['logs_bytes']))
            exact(expected,bodies)
        else:
            records,bodies=decode_records(work/'records.jsonl');exact(expected,bodies)
        verdicts=[]
        if case['mode']=='query':verdicts,_=grade_query(work,dest,expected,repeat==1 and variant=='plain',preflight)
        stats={}
        names=defaultdict(list)
        for r in rows:
            if 'wall_ns' in r:names[r['stage']].append(r)
        for name,samples in names.items():
            stats[name]={'wall_ns':quantiles([x['wall_ns'] for x in samples]),'cpu_ns':sum(x['cpu_ns'] for x in samples),'units':sum(x['units'] for x in samples),'allocation':[x['allocation'] for x in samples],'io':[x['proc_io_delta'] for x in samples]}
        summary={'case':case,'repeat':repeat,'variant':variant,'preflight':preflight,'exit':p.returncode,'complete':rows[-1],'exact_source_bodies':True,'source_bytes':rows[0]['source_bytes'],'fixture':rows[0],'stats':stats,'query_verdicts':verdicts,'elapsed_s':time.monotonic()-began}
        dump(dest/'summary.json',summary);dump(dest/'resources.json',mon)
        for name in ['timings.jsonl','phases.jsonl','journal-files-before.json']:
            if (work/name).exists():compress(work/name,dest/(name+'.gz'))
        # One content-addressed source archive per unique replay, no duplication
        # across worker/file coordinates; every trial checked before cleanup.
        if repeat==1 and variant=='plain':
            digest=hashlib.sha256((work/'records.jsonl').read_bytes()).hexdigest()
            blobs=root/'records';blobs.mkdir(exist_ok=True);archive=blobs/(digest+'.jsonl.gz')
            if not archive.exists():compress(work/'records.jsonl',archive)
            dump(dest/'records-archive.json',{'sha256':digest,'archive':str(archive.relative_to(root))})
        return summary
    except Exception as e:
        dump(dest/'failure.json',{'error':str(e),'exit':None if p is None else p.poll()})
        for f in work.rglob('*'):
            if f.is_file() and (f.name in ['records.jsonl','phases.jsonl','timings.jsonl'] or f.name.startswith('answer-')):
                compress(f,dest/('failure-'+f.name+'.gz'))
        raise
    finally:
        if p is not None and p.poll() is None:p.kill();p.wait()
        assert (work/'owned').read_text()==label
        amount=footprint(work);shutil.rmtree(work);dump(dest/'cleanup.json',{'owned':label,'logical_bytes':amount,'removed':not work.exists()})

def cases():
    out=[{'id':'assembly','mode':'assembly'},{'id':'assembly-invalid','mode':'assembly','bad':1},{'id':'processing','mode':'processing'},{'id':'compression','mode':'compression'},{'id':'query','mode':'query'}]
    # Independent granularity cuts first, then worker cuts, then interactions.
    coords=[(1,1),(16,1),(64,1),(64,2),(64,3),(64,4),(16,2),(16,3),(16,4)]
    return out+[{'id':f'granularity-{mb}-workers-{w}','mode':'scheduling','workers':w,'rotate':mb*2**20,'records':262144} for mb,w in coords]

def main():
    sys.path.insert(0, str(REPO/'tools'))
    from resource_group import require_limits
    require_limits()
    ap=argparse.ArgumentParser();ap.add_argument('--root',type=Path,required=True);ap.add_argument('--bins',type=Path,required=True);ap.add_argument('--phase',choices=['preflight','calibration','measure'],required=True);a=ap.parse_args();a.root.mkdir(parents=True,exist_ok=True)
    dump(a.root/'negative-controls.json',negative_controls());results=[]
    if a.phase=='preflight':
        for c in cases():
            for variant in ['plain','counted']:results.append(run(a.root,a.bins,c,1,variant,True))
    elif a.phase=='calibration':
        c={'id':'processing','mode':'processing'}
        for n in range(1,6):
            for v in (['plain','phases','counted'] if n%2 else ['counted','phases','plain']):results.append(run(a.root,a.bins,c,n,v))
    else:
        for c in cases():
            if c.get('bad'):continue
            for n in range(1,6):
                for v in (['plain','counted'] if n%2 else ['counted','plain']):results.append(run(a.root,a.bins,c,n,v))
    dump(a.root/'complete.json',{'phase':a.phase,'trials':len(results),'exit':0})
if __name__=='__main__':main()
