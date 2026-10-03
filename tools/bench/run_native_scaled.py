#!/usr/bin/env python3
"""Real Spindle aggregate-volume pilots; file feeder never sends Batches."""
import argparse, base64, gzip, hashlib, json, os, signal, socket, sqlite3, subprocess, sys, threading, time, shutil
from collections import Counter
from pathlib import Path
REPO=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(REPO/'tools/bench'));from run_dev_small import dump, process_stats, footprint, percentile
from run_scaled_fleet import weighted
sys.path.insert(0,str(REPO/'tools/qualification'));from delivery_faults import make_certs,free_port
import query_oracle
import ssl, urllib.request


def io_stats(pid):
    return {k:int(v) for k,v in (line.split(':') for line in Path(f'/proc/{pid}/io').read_text().splitlines())}


def trial(root,bins,name,per_tick,cpus):
    root.mkdir();(root/'owned').write_text('real Spindle scaled pilot\n');make_certs(root);port=free_port()
    admin=os.urandom(32).hex();(root/'admin').write_text(admin+'\n');ctx=ssl.create_default_context(cafile=str(root/'ca.pem'))
    conf=root/'server.conf';conf.write_text(f'listen=127.0.0.1:{port}\ntls_cert={root}/server.pem\ntls_key={root}/server.key\nstate_dir={root}/state\nadmin_token_file={root}/admin\njournal_bytes=1073741824\njournal_file_bytes=67108864\nretention_s=86400\nretention_bytes=8589934592\nseal_workers=1\n')
    kids=[];events={};readers=[];samples=[];errors=[];stop=threading.Event();began=time.monotonic();producer=None
    def spawn(binary,args,affinity,limit,label,piped=False):
        p=subprocess.Popen(['prlimit',f'--as={limit}','--','taskset','-c',','.join(map(str,affinity)),str(binary),*map(str,args)],stdout=subprocess.PIPE if piped else open(root/(label+'.out'),'wb'),stderr=open(root/(label+'.err'),'wb'),bufsize=0)
        kids.append(p);return p
    def reader(p,label):
        e=events[label]=[]
        with gzip.open(root/(label+'-events.jsonl.gz'),'wt') as f:
            for raw in iter(p.stdout.readline,b''):
                t=time.time_ns();line=raw.decode().strip()
                if line.startswith('batch=') or line.startswith('delivery '):
                    row={'t':t,'kind':'cycle' if line.startswith('batch=') else 'attempt',**dict(s.split('=',1) for s in line.split() if '=' in s)};e.append(row);f.write(json.dumps(row)+'\n')
    try:
        server=spawn(bins/'fabric-server',['serve',conf],cpus[:2],4*2**30,'server')
        for _ in range(200):
            if server.poll() is not None:raise RuntimeError('server startup exit')
            try:
                with socket.create_connection(('127.0.0.1',port),timeout=.1):break
            except OSError:time.sleep(.05)
        else:raise RuntimeError('server startup timeout')
        for i in range(20):
            label=f'node{i:02}';path=root/(label+'.log');path.touch()
            req=urllib.request.Request(f'https://127.0.0.1:{port}/v1/admin/nodes',data=json.dumps({'name':label,'metric_interval_s':15,'logs':[str(path)]}).encode(),headers={'authorization':'Bearer '+admin,'content-type':'application/json'},method='POST')
            with urllib.request.urlopen(req,context=ctx,timeout=10) as r:token=json.loads(r.read())['token']
            (root/(label+'.token')).write_text(token+'\n');nc=root/(label+'.conf');nc.write_text(f'spool_dir={root/(label+"-spool")}\nlog={path}\nmetric_interval_s=15\nspool_bytes=268435456\nserver_url=https://127.0.0.1:{port}\nserver_ca={root}/ca.pem\ntoken_file={root/(label+".token")}\n')
            p=spawn(bins/'fabric-node',['run',nc],cpus[2:4],512*2**20,label,True);t=threading.Thread(target=reader,args=(p,label));t.start();readers.append(t)
        nodes=kids[1:].copy()
        def sample():
            while not stop.is_set():
                try:
                    if time.monotonic()-began>1200:raise RuntimeError('wall bound')
                    size=footprint(root)
                    if size>12*2**30 or shutil.disk_usage(root).free<4*2**30:raise RuntimeError('timed disk bound')
                    if any(p.poll() is not None for p in [server,*nodes]):raise RuntimeError('native child exited early')
                    wall=time.time_ns();mono=time.monotonic_ns();last=[]
                    for i in range(20):
                        ev=events.get(f'node{i:02}',[]);cycles=[e for e in ev if e['kind']=='cycle'];acks=[e for e in ev if e['kind']=='attempt' and e['status']=='ack'];c=cycles[-1] if cycles else {}
                        last.append({'batch':int(c.get('batch',0)),'acked':int(acks[-1]['committed_through']) if acks else 0,'file_backlog':int(c.get('log_backlog_bytes',0)),'spool_bytes':int(c.get('spool_bytes',0))})
                    samples.append({'wall_ns':wall,'mono_ns':mono,'server':process_stats(server.pid),'nodes':[process_stats(p.pid) for p in nodes],'producer':process_stats(producer.pid) if producer is not None and producer.poll() is None else None,'progress':last,'server_io':io_stats(server.pid),'nodes_io':[io_stats(p.pid) for p in nodes],'logical_disk_bytes':size,'journal_bytes':footprint(root/'state/journal'),'spool_disk_bytes':sum(footprint(root/f'node{i:02}-spool') for i in range(20)),'sealed_files':len(list((root/'state/journal').glob('sealed-*.faj'))),'segments':len(list((root/'state/segments').glob('seg-*'))),'cgroup_memory_current':int(Path('/sys/fs/cgroup/memory.current').read_text())})
                except Exception as e:errors.append(str(e));stop.set();return
                stop.wait(1)
        sampler=threading.Thread(target=sample);sampler.start();stop.wait(5)
        producer=spawn(bins/'native_source',[root,str(per_tick)],cpus[2:4],512*2**20,'producer')
        while producer.poll() is None:
            if stop.wait(.2):raise RuntimeError('; '.join(errors))
            if time.monotonic()-began>300:raise RuntimeError('producer deadline')
        if producer.returncode:raise RuntimeError('producer failure')
        drain_begin=time.monotonic();drained=False
        while time.monotonic()-drain_begin<120:
            if stop.wait(1):raise RuntimeError('; '.join(errors))
            if samples and all(p['batch']>0 and p['acked']>=p['batch'] and p['file_backlog']==0 for p in samples[-1]['progress']):
                # Last observed cycle must postdate the final source write.
                last_write=max(int(line.split(',')[4]) for line in (root/'source-clocks.csv').read_text().splitlines())
                if all(max((e['t'] for e in events[f'node{i:02}'] if e['kind']=='cycle'),default=0)>last_write for i in range(20)):
                    drained=True;break
        drain_seconds=time.monotonic()-drain_begin;stop.wait(20)
        if errors:raise RuntimeError('; '.join(errors))
        stop.set();sampler.join(5)
        for p in nodes:p.send_signal(signal.SIGTERM)
        for p in nodes:p.wait(timeout=30)
        for t in readers:t.join(5)
        server.send_signal(signal.SIGTERM);server.wait(timeout=30);dump(root/'resources.json',samples)
        source_clocks={}
        for line in (root/'source-clocks.csv').read_text().splitlines():
            i,tick,count,sched,after,lag=map(int,line.split(','));source_clocks[(i,tick)]=(count,sched,after,lag)
        acks={};rtt=[];retries=Counter()
        for label,ev in events.items():
            for e in ev:
                if e['kind']=='attempt':
                    if e['status']=='ack':acks[(label,int(e['sequence']))]=e;rtt.append(int(e['elapsed_us'])/1000)
                    else:retries[e['status']]+=1
        with (root/'replay.jsonl').open('wb') as out:subprocess.run([str(bins/'examples/server_dump'),str(conf),'--records'],stdout=out,stderr=open(root/'dump.err','wb'),check=True,timeout=300)
        if footprint(root)>16*2**30 or shutil.disk_usage(root).free<4*2**30:raise RuntimeError('analysis disk bound')
        db=sqlite3.connect(root/'audit.sqlite');db.execute('PRAGMA journal_mode=OFF');db.execute('PRAGMA synchronous=OFF');db.execute('PRAGMA cache_size=-65536');db.execute('CREATE TABLE logs(tag TEXT PRIMARY KEY, hash BLOB) WITHOUT ROWID')
        groups=Counter();keys=set();hashes={};decoded=0;unexpected=0;gaps=[];byte_count=0;batch_seqs={}
        with (root/'replay.jsonl').open() as src,gzip.open(root/'batch-hashes.jsonl.gz','wt') as hout:
            for line in src:
                record=json.loads(line);raw=base64.b64decode(record['bytes']);b=query_oracle.decode_batch(raw);key=(record['label'],b['sequence'])
                if key in keys:raise RuntimeError('duplicate recovered Batch')
                keys.add(key);sha=hashlib.sha256(raw).hexdigest();hashes[key]=sha;byte_count+=len(raw);gaps.extend(b['gaps']);batch_seqs.setdefault(key[0],[]).append(key[1]);hout.write(json.dumps([*key,sha,len(raw),record['received_ns']])+'\n');a=acks.get(key);ack=int(a['t']) if a else None
                rows=[]
                for log in query_oracle.decode_logs_request(b['logs_bytes']):
                    decoded+=1;body=log['body'];tag=body.split(' ',1)[0].removeprefix('load-')
                    try:i,tick,j=map(int,tag.split(':'));count,sched,after,lag=source_clocks[(i,tick)];assert 0<=j<count and key[0]==f'node{i:02}'
                    except (ValueError,KeyError,AssertionError):unexpected+=1;continue
                    rows.append((tag,hashlib.sha256(body.encode()).digest()));phase='normal' if tick<100 else 'burst' if tick<150 else 'recovery';groups[(i,tick,log['observed_time_unix_nano'],record['received_ns'],ack,after,sched,phase)]+=1
                db.executemany('INSERT OR IGNORE INTO logs VALUES (?,?)',rows)
        db.commit();unique=db.execute('SELECT count(*) FROM logs').fetchone()[0];missing=changed=offered=0;source_hashes={}
        for i in range(20):
            whole=hashlib.sha256()
            with (root/f'node{i:02}.log').open('rb') as f:
                for line in f:
                    whole.update(line);offered+=1;body=line.rstrip(b'\n');tag=body.split(b' ',1)[0][5:].decode();row=db.execute('SELECT hash FROM logs WHERE tag=?',(tag,)).fetchone()
                    if row is None:missing+=1
                    elif row[0]!=hashlib.sha256(body).digest():changed+=1
            source_hashes[f'node{i:02}.log']=whole.hexdigest()
        dump(root/'source-file-hashes.json',source_hashes);db.close();populations={p:{k:[] for k in ['ingest','ack','source_ingest','scheduled_ingest','source_ack']} for p in ['normal','burst','recovery','all']}
        with gzip.open(root/'clock-groups.jsonl.gz','wt') as f:
            for group,n in groups.items():
                i,tick,collected,received,ack,after,sched,phase=group;f.write(json.dumps([*group,n])+'\n')
                for p in [phase,'all']:
                    for k,delta in [('ingest',received-collected),('source_ingest',received-after),('scheduled_ingest',received-sched)]:populations[p][k].append((delta/1e6,n))
                    if ack is not None:populations[p]['ack'].append(((ack-collected)/1e6,n));populations[p]['source_ack'].append(((ack-after)/1e6,n))
        stats={p:{k:weighted(v) for k,v in columns.items()} for p,columns in populations.items()};expected=20*per_tick*350
        server_peak=max(s['server']['hwm_kib'] for s in samples)/1024;node_peak=max(n['hwm_kib'] for s in samples for n in s['nodes'])/1024;wall=(samples[-1]['mono_ns']-samples[0]['mono_ns'])/1e9;offsets=[(s['wall_ns']-s['mono_ns'])/1e6 for s in samples];producer_samples=[s for s in samples if s['producer']];lag_stats=percentile([v[3]/1e6 for v in source_clocks.values()]);missing_ack=sum(key not in hashes or hashes[key]!=e['sha256'] for key,e in acks.items());contiguous=all(sorted(seq)==list(range(1,max(seq)+1)) for seq in batch_seqs.values())
        gates={'all_exact_source_logs':offered==expected==decoded==unique and missing==changed==unexpected==0,'no_gaps':not gaps,'ack_hash_custody':missing_ack==0 and len(acks)==len(keys),'contiguous_sequences':contiguous,'clean_exits':all(p.returncode==0 for p in kids),'drained_within_120s':drained,'producer_lag_p99_le_100ms':lag_stats['p99']<=100,'ingest_p99_le_1s':stats['all']['ingest']['p99'] is not None and stats['all']['ingest']['p99']<=1000 and stats['all']['ingest']['samples']==expected and stats['all']['ingest']['negative']==0,'ack_p99_le_1s':stats['all']['ack']['p99'] is not None and stats['all']['ack']['p99']<=1000 and stats['all']['ack']['samples']==expected,'source_ingest_p99_le_2s':stats['all']['source_ingest']['p99'] is not None and stats['all']['source_ingest']['p99']<=2000 and stats['all']['source_ingest']['samples']==expected,'server_rss_le_2gib':server_peak<=2048,'node_rss_le_64mib':node_peak<=64,'offset_range_le_5ms':max(offsets)-min(offsets)<=5}
        summary={'name':name,'real_spindles':20,'ordinary_events_s':20*per_tick*10,'burst_events_s':20*per_tick*30,'expected_logs':expected,'offered_logs':offered,'decoded_logs':decoded,'unique_logs':unique,'missing_logs':missing,'changed_logs':changed,'unexpected_logs':unexpected,'gaps':gaps,'acked_batches':len(acks),'recovered_batches':len(keys),'missing_or_changed_ack':missing_ack,'producer':json.loads((root/'producer.out').read_text()),'producer_lag_ms':lag_stats,'latency_ms':stats,'native_attempt_rtt_ms':percentile(rtt),'retries':dict(retries),'drain_seconds_after_producer':drain_seconds,'server_peak_rss_mib':server_peak,'node_peak_rss_mib':node_peak,'aggregate_nodes_peak_rss_mib':max(sum(n['rss_kib'] for n in s['nodes']) for s in samples)/1024,'producer_peak_rss_mib':max((s['producer']['hwm_kib']/1024 for s in producer_samples),default=None),'server_mean_cpu_equivalents':(samples[-1]['server']['cpu_s']-samples[0]['server']['cpu_s'])/wall,'nodes_mean_cpu_equivalents':(sum(n['cpu_s'] for n in samples[-1]['nodes'])-sum(n['cpu_s'] for n in samples[0]['nodes']))/wall,'encoded_batch_bytes':byte_count,'encoded_bytes_per_source_log':byte_count/offered,'max_file_backlog_bytes_per_node':max(p['file_backlog'] for s in samples for p in s['progress']),'max_reported_spool_bytes_per_node':max(p['spool_bytes'] for s in samples for p in s['progress']),'peak_spool_disk_bytes':max(s['spool_disk_bytes'] for s in samples),'max_sealed_files':max(s['sealed_files'] for s in samples),'segments_final':samples[-1]['segments'],'sealed_final':samples[-1]['sealed_files'],'peak_timed_disk_bytes':max(s['logical_disk_bytes'] for s in samples),'offset_range_ms':max(offsets)-min(offsets),'gates':gates,'passed':all(gates.values())}
        dump(root/'summary.json',summary);(root/'replay.jsonl').unlink();print(json.dumps({'name':name,'passed':summary['passed'],'gates':gates,'latency_ms':stats['all'],'server_rss_mib':server_peak,'drain_s':drain_seconds}),flush=True)
    finally:
        stop.set()
        for p in kids:
            if p.poll() is None:p.kill();p.wait(timeout=10)


def main():
    ap=argparse.ArgumentParser();ap.add_argument('--out',type=Path,required=True);ap.add_argument('--bin-dir',type=Path,required=True);a=ap.parse_args();root=a.out.resolve();bins=a.bin_dir.resolve();assert not root.exists();root.mkdir(parents=True);cpus=sorted(os.sched_getaffinity(0));assert len(cpus)>=4
    # Independent exact-byte control, before workloads.
    expected=hashlib.sha256(b'original').digest();assert expected==hashlib.sha256(b'original').digest();assert expected!=hashlib.sha256(b'changed').digest();assert {}.get('missing') is None
    dump(root/'negative-control.json',{'identical_accepted':True,'changed_rejected':True,'missing_rejected':True})
    dump(root/'environment.json',{'revision':subprocess.check_output(['git','rev-parse','HEAD'],cwd=REPO,text=True).strip(),'affinity':cpus,'cpu_max':Path('/sys/fs/cgroup/cpu.max').read_text().strip(),'memory_max':Path('/sys/fs/cgroup/memory.max').read_text().strip(),'uname':list(os.uname()),'harness_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),'binaries':{str(p.relative_to(bins)):hashlib.sha256(p.read_bytes()).hexdigest() for p in [bins/'fabric-server',bins/'fabric-node',bins/'native_source',bins/'examples/server_dump']}})
    os.sched_setaffinity(0,cpus[2:4])
    for name,per_tick in [('medium',50),('enterprise',500)]:
        try:trial(root/name,bins,name,per_tick,cpus)
        except Exception as e:dump(root/(name+'-failure.json'),{'error':str(e)});raise
    dump(root/'complete.json',{'sequential':['medium','enterprise']})
if __name__=='__main__':main()
