#!/usr/bin/env python3
"""Sequential native public-boundary trials, custody/oracle checks, owned cleanup."""
import argparse,base64,copy,gzip,hashlib,json,math,os,shutil,signal,ssl,subprocess,sys,time,urllib.request
from collections import Counter,defaultdict
from pathlib import Path
REPO=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(REPO/'tools/qualification'));import query_oracle
from delivery_faults import make_certs,free_port
sys.path.insert(0,str(REPO/'tools/bench'));from run_dev_small import process_stats


def dump(path,obj):path.write_text(json.dumps(obj,indent=2)+'\n')
def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def footprint(root):
    n=0
    for p in root.rglob('*'):
        try:
            if p.is_file():n+=p.stat().st_size
        except FileNotFoundError:pass
    return n

def exact(expected,actual):
    if len(actual)!=len(expected) or Counter(actual)!=Counter(expected):raise AssertionError('missing/duplicate/changed bodies')

def decode_records(path):
    records=[];bodies=[]
    for line in path.read_text().splitlines():
        r=json.loads(line);raw=bytes.fromhex(r['hex']);assert hashlib.sha256(raw).hexdigest()==r['sha256'];b=query_oracle.decode_batch(raw);assert not b['gaps']
        records.append({'label':r['label'],'received_ns':r['received_ns'],'bytes':base64.b64encode(raw).decode()})
        bodies.extend(l['body'] for l in query_oracle.decode_logs_request(b['logs_bytes']))
    return records,bodies

def quantiles(values):
    a=sorted(values);return {'samples':len(a),'p50':a[math.ceil(len(a)*.5)-1],'p99':a[math.ceil(len(a)*.99)-1],'max':a[-1],'sum':sum(a)}

def samples(pid):
    row={'mono_ns':time.monotonic_ns(),'process':process_stats(pid),'memory_current':int(Path('/sys/fs/cgroup/memory.current').read_text()),'memory_stat':{},'memory_events':{}}
    for f in ['memory.stat','memory.events']:
        row[f.replace('.','_')]={k:int(v) for k,v in (line.split() for line in Path('/sys/fs/cgroup',f).read_text().splitlines())}
    return row

def run(root,bins,mode,size,pair,variant,cpus):
    label=f'{mode}-{size}-p{pair}-{variant}';work=root/'scratch'/label;work.mkdir();(work/'owned').write_text(label);dest=root/'evidence'/label;dest.mkdir();free_before=shutil.disk_usage(work).free;assert free_before>4*2**30
    command=['prlimit',f'--as={2*2**30}','--','taskset','-c',','.join(map(str,cpus[:2])),str(bins/variant),str(work),mode,str(size)];server=None;server_stats=[]
    try:
        if mode=='delivery':
            make_certs(work);port=free_port();admin=os.urandom(32).hex();(work/'admin').write_text(admin);conf=work/'server.conf';conf.write_text(f'listen=127.0.0.1:{port}\ntls_cert={work}/server.pem\ntls_key={work}/server.key\nstate_dir={work}/state\nadmin_token_file={work}/admin\njournal_bytes=268435456\njournal_file_bytes=67108864\nseal_workers=1\n')
            server=subprocess.Popen(['taskset','-c',','.join(map(str,cpus[2:4])),str(bins/'fabric-server'),'serve',str(conf)],stdout=open(work/'server.out','wb'),stderr=open(work/'server.err','wb'))
            ctx=ssl.create_default_context(cafile=str(work/'ca.pem'));url=f'https://127.0.0.1:{port}'
            for _ in range(100):
                try:
                    req=urllib.request.Request(url+'/v1/admin/nodes',data=json.dumps({'name':'fixture','metric_interval_s':15,'logs':[]}).encode(),headers={'authorization':'Bearer '+admin,'content-type':'application/json'},method='POST')
                    with urllib.request.urlopen(req,context=ctx,timeout=1) as r:token=json.loads(r.read())['token']
                    break
                except urllib.error.URLError:
                    if server.poll() is not None:raise RuntimeError('server startup failed')
                    time.sleep(.05)
            else:raise RuntimeError('server startup timeout')
            (work/'token').write_text(token);command.extend([url,str(work/'ca.pem'),str(work/'token')]);server_stats.append(samples(server.pid))
        dump(dest/'command.json',command);start=time.monotonic();mon=[]
        with (work/'timings.jsonl').open('wb') as out,(dest/'stderr.txt').open('wb') as err:
            p=subprocess.Popen(command,stdout=out,stderr=err)
            while p.poll() is None:
                if time.monotonic()-start>180 or footprint(work)>2**30 or shutil.disk_usage(work).free<4*2**30:p.kill();p.wait();raise RuntimeError('child resource bound')
                try:mon.append(samples(p.pid))
                except (FileNotFoundError,ProcessLookupError):pass
                if server is not None:server_stats.append(samples(server.pid))
                time.sleep(.1)
        assert p.returncode==0,f'probe exit {p.returncode}'
        elapsed=time.monotonic()-start
        if server is not None:
            server_stats.append(samples(server.pid));server.send_signal(signal.SIGTERM);server.wait(timeout=30)
            with (work/'replay.jsonl').open('wb') as f:subprocess.run([str(bins/'server_dump'),str(work/'server.conf'),'--records'],stdout=f,stderr=open(dest/'dump.err','wb'),check=True,timeout=60)
        rows=[json.loads(line) for line in (work/'timings.jsonl').read_text().splitlines()];assert rows[-1]['stage']=='complete';meta=rows[0];source=(work/'source.log').read_bytes();assert hashlib.sha256(source).hexdigest()==meta['source_sha256'];expected=source.decode().splitlines();records,bodies=decode_records(work/'records.jsonl');exact(expected,bodies);oracle=[];controls={}
        if mode=='delivery':
            recovered=[];decoded=[]
            for line in (work/'replay.jsonl').read_text().splitlines():
                r=json.loads(line);raw=base64.b64decode(r['bytes']);b=query_oracle.decode_batch(raw);recovered.append((b['sequence'],hashlib.sha256(raw).hexdigest()));decoded.extend(l['body'] for l in query_oracle.decode_logs_request(b['logs_bytes']))
            observed=[(r['sequence'],r['sha256']) for r in rows if r['stage']=='acked_hash'];assert sorted(observed)==sorted(recovered);exact(expected,decoded);controls['exact_ack_custody']=True
        if mode=='query':
            for path in work.glob('answer-*.json'):
                obj=json.loads(path.read_text());kind=path.name.split('-')[1];r,b=decode_records(work/(kind+'-records')/'records.jsonl');exact(expected,b);verdict=query_oracle.check(r,obj['query'],[obj['answer']]);oracle.append({'file':path.name,'sha256':sha(path),'verdict':verdict});assert verdict['passed'],verdict
                if obj['answer']['rows'] and not controls:
                    bad=copy.deepcopy(obj['answer']);bad['rows'][0]['body']+='changed';assert not query_oracle.check(r,obj['query'],[bad])['passed'];bad=copy.deepcopy(obj['answer']);bad['rows']=bad['rows'][1:];assert not query_oracle.check(r,obj['query'],[bad])['passed'];controls={'changed_query_rejected':True,'missing_query_rejected':True}
                if pair==1 and variant=='plain':
                    with gzip.open(dest/(path.name+'.gz'),'wb') as f:f.write(path.read_bytes())
        stages=defaultdict(list)
        for row in rows:
            if 'wall_ns' in row:stages[row['stage']].append(row)
        stats={name:{'wall_ms':quantiles([r['wall_ns']/1e6 for r in v]),'units':sum(r['units'] for r in v),'cpu_ticks':sum(r['cpu_ticks'] for r in v),'proc_io':{k:sum(r['proc_io_delta'][k] for r in v) for k in v[0]['proc_io_delta']},'max_baseline_live_bytes':max((r['allocation']['baseline_live_bytes'] for r in v if r['allocation']),default=None),'max_peak_live_bytes':max((r['allocation']['peak_live_bytes'] for r in v if r['allocation']),default=None),'max_incremental_peak_bytes':max((r['allocation']['incremental_peak_bytes'] for r in v if r['allocation']),default=None),'cumulative_requested_bytes':sum((r['allocation']['cumulative_requested_bytes'] for r in v if r['allocation']),0)} for name,v in stages.items()}
        summary={'label':label,'mode':mode,'size':size,'pair':pair,'variant':variant,'elapsed_process_s':elapsed,'exit':p.returncode,'source_sha256':meta['source_sha256'],'records_sha256':sha(work/'records.jsonl'),'exact_fixture_bodies':True,'oracle':oracle,'controls':controls,'stats':stats,'sampled_peak_process_hwm_mib':max((s['process']['hwm_kib']/1024 for s in mon),default=None),'sample_count':len(mon),'cpu_tick_hz':os.sysconf('SC_CLK_TCK')}
        dump(dest/'summary.json',summary);dump(dest/'resources.json',mon);dump(dest/'server-resources.json',server_stats)
        with gzip.open(dest/'timings.jsonl.gz','wb') as f:f.write((work/'timings.jsonl').read_bytes())
        if pair==1 and variant=='plain':
            with gzip.open(dest/'records.jsonl.gz','wb') as f:f.write((work/'records.jsonl').read_bytes())
        return summary
    except Exception as e:
        dump(dest/'failure.json',{'error':str(e)})
        if (work/'timings.jsonl').exists():
            with gzip.open(dest/'partial-timings.jsonl.gz','wb') as f:f.write((work/'timings.jsonl').read_bytes())
        raise
    finally:
        if server is not None and server.poll() is None:server.kill();server.wait(timeout=10)
        # Failure evidence was saved above; remove only this runner's marked trial.
        removed=footprint(work);assert (work/'owned').read_text()==label;shutil.rmtree(work)
        dump(dest/'cleanup.json',{'owned_directory':str(work),'removed_logical_bytes':removed,'free_before':free_before,'free_after':shutil.disk_usage(root).free,'removed':not work.exists()})

def main():
    ap=argparse.ArgumentParser();ap.add_argument('--out',type=Path,required=True);ap.add_argument('--bins',type=Path,required=True);a=ap.parse_args();root=a.out.resolve();bins=a.bins.resolve();assert not root.exists();root.mkdir();(root/'scratch').mkdir();(root/'evidence').mkdir();cpus=sorted(os.sched_getaffinity(0));assert len(cpus)>=4
    for altered in [['original','changed'],['original']]:
        try:exact(['original','second'],altered)
        except AssertionError:pass
        else:raise AssertionError('negative control accepted')
    dump(root/'controls.json',{'changed_body_rejected':True,'missing_body_rejected':True})
    dump(root/'environment.json',{'revision':subprocess.check_output(['git','rev-parse','HEAD'],cwd=REPO,text=True).strip(),'affinity':cpus,'cpu_max':Path('/sys/fs/cgroup/cpu.max').read_text().strip(),'memory_max':Path('/sys/fs/cgroup/memory.max').read_text().strip(),'binaries':{p.name:sha(p) for p in bins.iterdir() if p.is_file()},'harness_sha256':sha(Path(__file__)),'uname':list(os.uname())})
    all_results=[];began=time.monotonic()
    for mode in ['collection','delivery','storage','processing','query','control','scheduling','observation']:
        for size in ([1,20,200] if mode=='control' else [1,2] if mode=='scheduling' else [900] if mode=='observation' else [128,900,3500]):
            for pair in [1,2,3]:
                for variant in (['plain','counted'] if pair%2 else ['counted','plain']):
                    assert time.monotonic()-began<3600
                    s=run(root,bins,mode,size,pair,variant,cpus);all_results.append(s);print(json.dumps({'trial':s['label'],'exit':s['exit'],'elapsed_s':s['elapsed_process_s'],'cleanup':True}),flush=True)
                    assert footprint(root/'evidence')<=256*2**20
        dump(root/(mode+'-complete.json'),{'trials':[s['label'] for s in all_results if s['mode']==mode]})
    dump(root/'summary.json',all_results);dump(root/'complete.json',{'trials':len(all_results),'seconds':time.monotonic()-began,'scratch_empty':not any((root/'scratch').iterdir())})
if __name__=='__main__':main()
