"""Private native signal/lifecycle fixture; frozen independent query oracle."""
import argparse, base64, gzip, hashlib, json, os, shutil, signal, socket, ssl
import subprocess, sys, threading, time, urllib.request
from pathlib import Path
ROOT=Path(__file__).resolve().parents[4]
sys.path.insert(0,str(ROOT/'tools'))
sys.path.insert(0,str(ROOT/'tools/qualification'))
from resource_group import require_limits, STORAGE
from delivery_faults import free_port, make_certs
import query_oracle
import cgroups
DATA=ROOT/'docs/experiments/benchmarks/data/lab-completion-run-01'

def dump(p,v): p.write_text(json.dumps(v,indent=2)+'\n')
def sha(raw): return hashlib.sha256(raw).hexdigest()
def pct(rows):
    rows=sorted(rows)
    return {'n':len(rows),'p50':rows[len(rows)//2] if rows else None,
            'p99':rows[min(len(rows)-1,int(len(rows)*.99))] if rows else None,
            'max':max(rows) if rows else None}
def exact(expected,actual):
    return len(actual)==len(set(actual)) and set(expected)==set(actual)

def main():
    require_limits()
    ap=argparse.ArgumentParser(); ap.add_argument('--id',required=True)
    ap.add_argument('--profile',choices=['development','small'],default='development')
    ap.add_argument('--signals',choices=['trace','logs','mixed'],default='mixed')
    ap.add_argument('--duration',type=int,default=180); ap.add_argument('--limits',action='store_true')
    ap.add_argument('--pressure',action='store_true')
    ap.add_argument('--bin-dir',type=Path)
    ap.add_argument('--query-plan',choices=['scan','walk'],default='scan')
    ap.add_argument('--lifecycle',action='store_true'); ap.add_argument('--controls',action='store_true')
    a=ap.parse_args()
    if a.controls:
        assert exact(['a'],['a'])
        assert not exact(['a'],[]) and not exact(['a'],['a','a']) and not exact(['a'],['b'])
        print('exact source controls rejected loss, duplicates and alteration'); return
    if a.duration not in (30,180,12000): raise RuntimeError('unregistered duration')
    if a.pressure and (not a.limits or a.duration!=180 or a.signals!='mixed'):
        raise RuntimeError('pressure requires finite mixed deployment limits')
    if a.lifecycle and (a.duration!=12000 or a.signals!='logs' or a.profile!='development'):
        raise RuntimeError('lifecycle must preserve registered development duty cycle')
    out=DATA/('memory' if a.lifecycle or a.limits else 'recovery')/a.id
    out.mkdir(parents=True,exist_ok=False)
    work=Path(os.environ['FABRIC_SCRATCH_ROOT'])/a.id; work.mkdir()
    if not work.resolve().is_relative_to(STORAGE/'scratch'): raise RuntimeError('owned data drive required')
    bins=a.bin_dir.resolve(strict=True) if a.bin_dir else Path(os.environ['CARGO_TARGET_DIR'])/'release'
    cpus=sorted(os.sched_getaffinity(0)); nodes=1 if a.profile=='development' else 20
    groups={}; parent=None; group_receipts={}
    if a.limits:
        parent=cgroups.delegate()
        host,_=cgroups.subgroup(parent,'server-host',3328*2**20 if nodes>1 else 640*2**20,
                                2816*2**20 if nodes>1 else 480*2**20,640,4,True)
        groups['server'],group_receipts['server']=cgroups.subgroup(host,'server',
                (3072 if nodes>1 else 512)*2**20,(2560 if nodes>1 else 384)*2**20,512,2)
        for i in range(nodes):
            node_parent=host if i==0 else parent
            groups[f'node{i:02}'],group_receipts[f'node{i:02}']=cgroups.subgroup(node_parent,f'node{i:02}',
                (256 if nodes>1 else 128)*2**20,(192 if nodes>1 else 96)*2**20,128,1)
    dump(out/'environment.json',{'argv':sys.argv,'binaries':{n:sha((bins/n).read_bytes()) for n in
        ['fabric-server','fabric-node','examples/server_dump']},'cpus':cpus,'groups':group_receipts,
        'scratch':str(work),'node_count':nodes,'source_seed':2703163393,
        'sdk_rate_pairs_per_s':10 if nodes==1 else 50,'system':list(os.uname())})
    make_certs(work); port=free_port(); otlp=free_port(); token=os.urandom(32).hex()
    (work/'admin-token').write_text(token+'\n')
    conf=work/'server.conf'
    conf.write_text(f'listen=127.0.0.1:{port}\ntls_cert={work}/server.pem\ntls_key={work}/server.key\nstate_dir={work}/state\nadmin_token_file={work}/admin-token\njournal_bytes=1073741824\njournal_file_bytes=67108864\nretention_s=86400\nretention_bytes=1073741824\nseal_workers=1\nquery_plan={a.query_plan}\n')
    if a.pressure:conf.write_text(conf.read_text().replace('journal_bytes=1073741824','journal_bytes=1048576').replace('journal_file_bytes=67108864','journal_file_bytes=65536'))
    ctx=ssl.create_default_context(cafile=str(work/'ca.pem'))
    def api(endpoint,body):
        req=urllib.request.Request(f'https://127.0.0.1:{port}'+endpoint,data=json.dumps(body).encode(),
            headers={'authorization':'Bearer '+token,'content-type':'application/json'})
        with urllib.request.urlopen(req,context=ctx,timeout=30) as r:return json.loads(r.read())
    children=[]; threads=[]; events=[]; query_samples=[]; files=[]; errors=[]; stop=threading.Event()
    server=None; sdk=None; success=False; outage=threading.Event(); restart_thread=None
    def spawn(name,argv,assigned=None):
        command=['taskset','-c',','.join(map(str,cpus[:2] if name=='server' else cpus[2:4])),*map(str,argv)]
        if assigned:
            command=[sys.executable,str(Path(__file__).with_name('enter_group.py')),str(assigned),*command]
        err=open(work/(name+'.err'),'ab'); files.append(err)
        p=subprocess.Popen(command,stdout=subprocess.PIPE,stderr=err); children.append(p)
        def reader():
            with gzip.open(out/(name+f'-{p.pid}.stdout.gz'),'wt') as f:
                for raw in iter(p.stdout.readline,b''):
                    line=raw.decode().strip(); now=time.time_ns()
                    f.write(json.dumps({'wall_ns':now,'line':line})+'\n'); f.flush()
                    if line.startswith('delivery ') or line.startswith('batch='):
                        fields=dict(x.split('=',1) for x in line.split() if '=' in x)
                        events.append({'label':name,'wall_ns':now,'kind':'ack' if line.startswith('delivery ') else 'cycle',**fields})
        t=threading.Thread(target=reader);t.start();threads.append(t);return p
    def server_start():
        p=spawn('server',[bins/'fabric-server','serve',conf],groups.get('server'))
        for _ in range(200):
            if p.poll() is not None:raise RuntimeError('server exited during startup')
            try:
                with socket.create_connection(('127.0.0.1',port),timeout=.1):return p
            except OSError:time.sleep(.05)
        raise RuntimeError('server startup timeout')
    try:
        server=server_start(); logs=[]; node_kids=[]
        for i in range(nodes):
            label=f'node{i:02}'; path=work/(label+'.log'); path.touch()
            log=open(path,'ab',buffering=0);logs.append(log);files.append(log)
            cred=api('/v1/admin/nodes',{'name':label,'metric_interval_s':15,'logs':[] if a.signals=='trace' else [str(path)]})['token']
            (work/(label+'.token')).write_text(cred+'\n')
            nc=work/(label+'.conf')
            nc.write_text(f'spool_dir={work}/{label}-spool\nmetric_interval_s=15\nspool_bytes=268435456\nserver_url=https://127.0.0.1:{port}\nserver_ca={work}/ca.pem\ntoken_file={work}/{label}.token\n'+
                ('' if a.signals=='trace' else f'log={path}\n')+
                (f'traces_listen=127.0.0.1:{otlp}\n' if i==0 and a.signals!='logs' else ''))
            if a.pressure:nc.write_text(nc.read_text().replace('spool_bytes=268435456','spool_bytes='+str(65536 if nodes==1 else 1048576)))
            node_kids.append(spawn(label,[bins/'fabric-node','run',nc],groups.get(label)))
        time.sleep(5)
        epoch=time.monotonic(); epoch_wall=time.time_ns(); expected=[]; transitions=[]; max_segments=0
        if a.signals!='logs':
            sdk=spawn('sdk',[STORAGE/'sdk-completion-1.38.0/bin/python',Path(__file__).with_name('sdk_source.py'),
                '--endpoint',f'http://127.0.0.1:{otlp}/v1/traces','--output',work/'sdk',
                '--duration',a.duration,'--rate',10 if nodes==1 else 50])
        # Query client timing is descriptive. Final full chains use unchanged oracle.
        def queries():
            while not stop.wait(10):
                kinds=['logs'] if a.signals=='logs' else ['spans'] if a.signals=='trace' else ['logs','spans']
                for kind in kinds:
                    q={'kind':kind,'from_ns':time.time_ns()-20_000_000_000 if kind=='spans' else 0,
                       'to_ns':2**63-1,'limit':10000 if kind=='spans' else 100}
                    began=time.monotonic_ns()
                    try:
                        answer=api('/v1/admin/query',q)
                        query_samples.append({'start_ns':began,'end_wall_ns':time.time_ns(),
                            'elapsed_ms':(time.monotonic_ns()-began)/1e6,'query':q,'answer':answer})
                    except Exception as exc:query_samples.append({'start_ns':began,'query':q,'error':repr(exc)})
        qt=threading.Thread(target=queries);qt.start()
        with gzip.open(out/'sources.jsonl.gz','wt') as source, gzip.open(out/'lifecycle.jsonl.gz','wt') as lifecycle:
            for tick in range(a.duration*10):
                time.sleep(max(0,epoch+tick*.1-time.monotonic()))
                if any(p.poll() is not None for p in node_kids) or (server.poll() is not None and not outage.is_set()):raise RuntimeError('native process exited early')
                if sdk and sdk.poll() is not None and sdk.returncode:raise RuntimeError('SDK source failed')
                phase=min(2,tick//600) if not a.lifecycle else 0
                count=(1 if nodes==1 else 5)*(3 if phase==1 else 1)
                if a.signals!='trace':
                    for i,log in enumerate(logs):
                        for j in range(count):
                            tag=f'{i:02}:{tick:06}:{j:02}'; padding='R'*900 if (tick+j)%2==0 else base64.b85encode(hashlib.shake_256(f'2703163393:{tag}'.encode()).digest(720)).decode()
                            body=('load-'+tag+' '+padding)[:900];log.write((body+'\n').encode());expected.append(sha(body.encode()))
                            source.write(json.dumps({'tag':tag,'sha256':expected[-1],'wall_ns':time.time_ns(),'phase':phase,'late_ms':max(0,time.monotonic()-epoch-tick*.1)*1000})+'\n')
                if tick%50==0:
                    labels=sorted(p.name for p in (work/'state/segments').glob('seg-*'))
                    max_segments=max(max_segments,len(labels)); journal=work/'state/journal'
                    row={'wall_ns':time.time_ns(),'mono_ns':time.monotonic_ns(),'boottime_ns':time.clock_gettime_ns(time.CLOCK_BOOTTIME),
                         'segments':labels,'sealed':len(list(journal.glob('sealed-*'))),
                         'active_bytes':sum(p.stat().st_size for p in journal.glob('active*'))}
                    spool={}
                    for i in range(nodes):
                        size=0
                        for p in (work/f'node{i:02}-spool').rglob('*'):
                            try:
                                if p.is_file():size+=p.stat().st_size
                            except FileNotFoundError:pass
                        spool[f'node{i:02}']=size
                    row['spool_disk_bytes']=spool
                    if parent:row['application_cgroups']=cgroups.snapshot(parent)
                    lifecycle.write(json.dumps(row)+'\n');lifecycle.flush()
                # A finite server restart tests custody while native nodes buffer.
                if a.pressure and tick==600:
                    outage.set();server.send_signal(signal.SIGTERM);server.wait(timeout=30)
                    def restart():
                        nonlocal server
                        try:
                            time.sleep(45);server=server_start();outage.clear()
                            transitions.append({'kind':'pressure_reconnect','wall_ns':time.time_ns()})
                        except Exception as exc:errors.append(repr(exc))
                    restart_thread=threading.Thread(target=restart);restart_thread.start()
                    transitions.append({'kind':'pressure_disconnect','wall_ns':time.time_ns()})
                elif not a.lifecycle and not a.pressure and tick==a.duration*5:
                    server.send_signal(signal.SIGTERM);server.wait(timeout=30);time.sleep(2);server=server_start()
                    transitions.append({'kind':'server_restart','wall_ns':time.time_ns()})
        if sdk and sdk.wait(timeout=40)!=0:raise RuntimeError('SDK failed flush/shutdown')
        if restart_thread:restart_thread.join(60)
        if errors:raise RuntimeError('; '.join(errors))
        time.sleep(20);stop.set();qt.join(35)
        for p in node_kids:p.send_signal(signal.SIGTERM)
        for p in node_kids:
            if p.wait(timeout=30):raise RuntimeError('unclean node shutdown')
        queries_by_kind={}
        for kind in ('logs','metrics','spans'):
            q={'kind':kind,'from_ns':0,'to_ns':2**63-1,'limit':10000}; pages=[];request=dict(q)
            if kind=='logs':q.update(node='node00',contains='load-00:000');request=dict(q)
            if kind=='metrics':q['name']='system.cpu.time';request=dict(q)
            for _ in range(1000):
                answer=api('/v1/admin/query',request);pages.append(answer)
                if not answer.get('next_page'):break
                request['page']=answer['next_page']
            else:raise RuntimeError('pagination did not terminate')
            queries_by_kind[kind]=(q,pages)
        server.send_signal(signal.SIGTERM)
        if server.wait(timeout=30):raise RuntimeError('unclean server shutdown')
        with (work/'recovered.jsonl').open('wb') as f:
            subprocess.run([bins/'examples/server_dump',conf,'--records'],stdout=f,check=True,timeout=240)
        records=[json.loads(line) for line in (work/'recovered.jsonl').read_text().splitlines()]
        got_logs=[];trace_bytes=bytearray();spans=[];batches={};batch_sizes={};metrics=0
        for r in records:
            raw=base64.b64decode(r['bytes']);b=query_oracle.decode_batch(raw);key=(r['label'],b['sequence'])
            if key in batches:raise RuntimeError('duplicate batch')
            batches[key]=sha(raw);batch_sizes[key]=len(raw);got_logs += [sha(x['body'].encode()) for x in query_oracle.decode_logs_request(b['logs_bytes'])]
            trace_bytes.extend(b['traces_bytes']);spans.extend(query_oracle.decode_traces_request(b['traces_bytes']))
            metrics+=len(query_oracle.decode_metrics_request(b['metrics_bytes']))
        verdicts={kind:query_oracle.check(records,q,pages) for kind,(q,pages) in queries_by_kind.items()}
        dump(out/'query-verdicts.json',verdicts)
        for kind,(q,pages) in queries_by_kind.items():
            with gzip.open(out/(kind+'-pages.json.gz'),'wt') as f:json.dump({'query':q,'pages':pages},f)
        acks={(e['label'],int(e['sequence'])):e for e in events if e['kind']=='ack' and e.get('status')=='ack'}
        gates={'exact_logs':exact(expected,got_logs),'ack_custody':set(acks)==set(batches) and all(batches[k]==v['sha256'] for k,v in acks.items()),
               'queries':all(v['passed'] for v in verdicts.values()),'metrics_present':metrics>0}
        with gzip.open(out/'lifecycle.jsonl.gz','rt') as f: clocks=[json.loads(line) for line in f]
        offsets=[r['wall_ns']-r['mono_ns'] for r in clocks]
        suspend=[r['boottime_ns']-r['mono_ns'] for r in clocks]
        clock={'realtime_offset_range_ns':max(offsets)-min(offsets),
               'suspend_offset_range_ns':max(suspend)-min(suspend)}
        clock['valid']=max(clock.values())<=5_000_000
        dump(out/'clock.json',clock)
        # Preserve correctness independently if the host clock invalidates timing.
        gates['clock_valid']=clock['valid']
        sdk_summary=None;local_ms=[]
        if sdk:
            attempts=[json.loads(x) for x in (work/'sdk/attempts.jsonl').read_text().splitlines()]
            exported=b''.join((work/'sdk'/x['body_file']).read_bytes() for x in attempts if x['response_status']==200)
            source=[json.loads(x) for x in (work/'sdk/source-spans.jsonl').read_text().splitlines()]
            if a.pressure:
                accepted_ids={(x['trace_id'],x['span_id']) for x in query_oracle.decode_traces_request(exported)}
                rejected=[x for x in source if (x['trace_id'],x['span_id']) not in accepted_ids]
                dump(out/'unaccepted-sdk-spans.json',{'count':len(rejected),'spans':rejected,
                    'interpretation':'offered but no recorded successful local Spool HTTP response; distinct from lost acknowledged custody'})
                source=[x for x in source if (x['trace_id'],x['span_id']) in accepted_ids]
            fields=('trace_id','span_id','parent_span_id','name','start_ns','end_ns')
            gates['raw_trace_bytes']=exported==trace_bytes
            gates['source_spans']=len(source)==len(spans) and sorted(tuple(x[k] for k in fields) for x in source)==sorted(tuple(x[k] for k in fields) for x in spans)
            sdk_summary=json.loads((work/'sdk/summary.json').read_text())
            gates['sdk_flush']=sdk_summary['force_flush'] is True and sdk_summary['error'] is None
            local_ms=[(x['end_mono_ns']-x['start_mono_ns'])/1e6 for x in attempts if x['response_status']==200]
            visibility={}
            for sample in query_samples:
                if sample.get('query',{}).get('kind')=='spans' and 'answer' in sample:
                    for span in sample['answer']['rows']:
                        visibility.setdefault((span['trace_id'],span['span_id']),sample['end_wall_ns'])
            span_clocks={}
            for r in records:
                b=query_oracle.decode_batch(base64.b64decode(r['bytes']));ack=acks.get((r['label'],b['sequence']))
                for span in query_oracle.decode_traces_request(b['traces_bytes']):
                    key=(span['trace_id'],span['span_id'])
                    span_clocks[key]={'trace_id':key[0],'span_id':key[1],'sequence':b['sequence'],
                        'server_received_ns':r['received_ns'],'server_ack_observed_ns':ack['wall_ns'] if ack else None,
                        'first_query_observed_ns':visibility.get(key)}
            for attempt in attempts:
                if attempt['response_status']==200:
                    for span in query_oracle.decode_traces_request((work/'sdk'/attempt['body_file']).read_bytes()):
                        span_clocks[(span['trace_id'],span['span_id'])]['local_spool_response_ns']=attempt['end_wall_ns']
            with gzip.open(out/'span-clocks.jsonl.gz','wt') as f:
                for row in span_clocks.values():f.write(json.dumps(row)+'\n')
            shutil.copytree(work/'sdk',out/'sdk')
        if a.lifecycle:gates['two_natural_rotations']=max_segments>=2 and not list((work/'state/journal').glob('sealed-*'))
        if parent:
            charges=cgroups.snapshot(parent);dump(out/'application-cgroups.json',charges)
            gates['no_application_oom']=all('oom_kill 0' in row['memory.events'] for row in charges.values())
        # Observation timestamps delimit offered work, excluding startup/drain.
        # Cycle lines cover native logs/metrics collection, not trace HTTP acceptance.
        offered_end=epoch_wall+a.duration*1_000_000_000
        offered_events=[e for e in events if epoch_wall<=e['wall_ns']<offered_end]
        cycles={(e['label'],int(e['batch'])) for e in offered_events if e['kind']=='cycle'}
        delivery=[e for e in offered_events if e['kind']=='ack']
        observed_acks={(e['label'],int(e['sequence'])) for e in delivery if e.get('status')=='ack'}
        rates={'interval_s':a.duration,'clock_valid':clock['valid'],
               'native_collection_accepted_batches':len(cycles),
               'native_collection_accepted_batches_per_s':len(cycles)/a.duration,
               'native_collection_accepted_encoded_bytes_per_s':sum(batch_sizes[k] for k in cycles if k in batch_sizes)/a.duration,
               'native_collection_missing_recovery_keys':len(cycles-set(batch_sizes)),
               'delivery_attempts':len(delivery),'delivery_non_ack_attempts':sum(e.get('status')!='ack' for e in delivery),
               'unique_server_acks_per_s':len(observed_acks)/a.duration,
               'server_acked_encoded_bytes_per_s':sum(batch_sizes[k] for k in observed_acks if k in batch_sizes)/a.duration,
               'ack_attempt_latency_ms':pct([int(e['elapsed_us'])/1000 for e in delivery if e.get('status')=='ack']),
               'scope':'Observer arrival within offered interval; native collection excludes trace-only Spool acceptance; failed collection denominator unavailable'}
        dump(out/'rates.json',rates)
        dump(out/'summary.json',{'gates':gates,'passed':all(gates.values()),'duration_s':a.duration,
             'logs':len(got_logs),'spans':len(spans),'metric_series':metrics,'batches':len(batches),'acks':len(acks),
             'sdk_local_response_ms':pct(local_ms),'query_latency_ms':pct([x['elapsed_ms'] for x in query_samples if 'elapsed_ms' in x]),
             'query_errors':[x for x in query_samples if 'error' in x],'sdk':sdk_summary,'transitions':transitions,
             'segments_observed':max_segments,'epoch_wall_ns':epoch_wall,'measured_wall_s':time.monotonic()-epoch})
        with gzip.open(out/'recovered-hashes.jsonl.gz','wt') as hashes, gzip.open(out/'signal-payloads.jsonl.gz','wt') as payloads:
            for r in records:
                raw=base64.b64decode(r['bytes']);b=query_oracle.decode_batch(raw)
                hashes.write(json.dumps({'label':r['label'],'sequence':b['sequence'],'received_ns':r['received_ns'],
                    'bytes':len(raw),'sha256':sha(raw)})+'\n')
                payloads.write(json.dumps({'label':r['label'],'sequence':b['sequence'],'received_ns':r['received_ns'],
                    'metrics_base64':base64.b64encode(b['metrics_bytes']).decode(),
                    'traces_base64':base64.b64encode(b['traces_bytes']).decode()})+'\n')
        with gzip.open(out/'events.jsonl.gz','wt') as f:
            for e in events:f.write(json.dumps(e)+'\n')
        with gzip.open(out/'queries.jsonl.gz','wt') as f:
            for q in query_samples:f.write(json.dumps(q)+'\n')
        evidence_bytes=sum(p.stat().st_size for p in out.parent.rglob('*') if p.is_file())
        if evidence_bytes>256*2**20:raise RuntimeError('lab evidence limit exceeded; preserve scratch')
        success=all(gates.values())
        if not success:raise RuntimeError('signal/lifecycle gate failed: '+json.dumps(gates))
    except BaseException as error:
        dump(out/'failure.json',{'error':repr(error),'scratch':str(work)});raise
    finally:
        stop.set()
        for p in children:
            if p.poll() is None:p.kill();p.wait(timeout=10)
        for t in threads:t.join(5)
        for f in files:f.close()
        if parent:dump(out/'final-cgroups.json',cgroups.snapshot(parent))
        if success:shutil.rmtree(work)
        dump(out/'cleanup.json',{'removed':not work.exists(),'scratch':str(work)})
if __name__=='__main__':main()
