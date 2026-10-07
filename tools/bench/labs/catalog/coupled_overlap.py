#!/usr/bin/env python3
"""O8 finite native TLS-delay screen; root registers/admit jobs, never qualification."""
import argparse
import base64
import gzip
import hashlib
import http.server
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import ssl
import statistics
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT/'tools'))
from resource_group import require_limits, STORAGE
sys.path.insert(0, str(ROOT/'tools/qualification'))
from delivery_faults import make_certs, free_port
import query_oracle
sys.path.insert(0, str(ROOT/'tools/bench/labs/completion'))
import cgroups
spec=importlib.util.spec_from_file_location('profile',ROOT/'tools/bench/labs/completion/profile.py')
profile=importlib.util.module_from_spec(spec);spec.loader.exec_module(profile)

def dump(path, value):
    path.write_text(json.dumps(value,indent=2)+'\n')

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def summary(values):
    values=sorted(values)
    return {'samples':len(values),'median_ms':statistics.median(values) if values else None,
            'max_ms':max(values) if values else None,'negative':sum(v<0 for v in values)}

def freeze(out,work,deadline):
    paths=['src/spindle/runtime.rs','src/spindle/overlap_tests.rs','src/spindle/sender.rs',
        'src/spindle/spool.rs','src/spindle/meter.rs','examples/coupled_overlap_node.rs',
        'crates/fabric-server/src/query.rs','crates/fabric-server/src/store.rs',
        'crates/fabric-server/src/segment.rs','crates/fabric-server/src/segment/bounded.rs',
        'crates/fabric-server/src/sealer.rs',
        'tools/bench/labs/catalog/coupled_overlap.py','tools/qualification/query_oracle.py',
        'tools/qualification/delivery_faults.py','tools/bench/labs/completion/cgroups.py',
        'docs/experiments/benchmarks/coupled-overlap-protocol.md','Cargo.lock']
    hashes={p:sha(ROOT/p) for p in paths}
    receipt={'status':'interrupted','source_hashes':hashes,'binaries':{},'commands':[],
        'environment':{'FABRIC_BORROWED_LOG_EXPERIMENT':'0',
            'FABRIC_SPILL_WORKSPACE_EXPERIMENT':None,'FABRIC_RUN_MIB_EXPERIMENT':None},'argv':sys.argv}
    dump(out/'freeze.json',receipt)
    env=dict(os.environ,FABRIC_BORROWED_LOG_EXPERIMENT='0')
    for key in ('FABRIC_SPILL_WORKSPACE_EXPERIMENT','FABRIC_RUN_MIB_EXPERIMENT'):
        env.pop(key,None)
    builds=[['cargo','build','--offline','--locked','--release','-p','fabric_o11y','--example','coupled_overlap_node'],
        ['cargo','build','--offline','--locked','--release','-p','fabric-server','--bin','fabric-server','--example','server_dump']]
    for index,argv in enumerate(builds):
        rc=profile.run_child(argv,env,out/f'build{index}.stdout',out/f'build{index}.stderr',deadline,work,out)
        receipt['commands'].append({'argv':argv,'exit':rc});dump(out/'freeze.json',receipt)
        if rc:raise RuntimeError('frozen build failed')
    if hashes!={p:sha(ROOT/p) for p in paths}:raise RuntimeError('source changed during freeze')
    for name,relative in [('node','examples/coupled_overlap_node'),('server','fabric-server'),('dump','examples/server_dump')]:
        source=Path(os.environ['CARGO_TARGET_DIR'])/'release'/relative
        archive=out/f'{name}.gz'
        with source.open('rb') as src,archive.open('xb') as dst:
            with gzip.GzipFile(fileobj=dst,mode='wb',mtime=0,filename='') as zipped:shutil.copyfileobj(src,zipped)
        readback=work/name
        with gzip.open(archive,'rb') as src,readback.open('xb') as dst:shutil.copyfileobj(src,dst)
        if source.read_bytes()!=readback.read_bytes() or sha(source)!=sha(readback):raise RuntimeError('binary archive drift')
        receipt['binaries'][name]={'archive':archive.name,'archive_sha256':sha(archive),
            'decoded_sha256':sha(readback),'decoded_bytes':readback.stat().st_size}
    receipt['status']='complete';dump(out/'freeze.json',receipt)

def trial(work,out,mode,bins,parent,deadline):
    work.mkdir();out.mkdir();make_certs(work)
    port,relay_port=free_port(),free_port()
    token='o8-admin-fixture-0123456789abcdef0123456789abcdef';(work/'admin-token').write_text(token+'\n')
    conf=work/'server.conf'
    conf.write_text(f'listen=127.0.0.1:{port}\ntls_cert={work}/server.pem\ntls_key={work}/server.key\nstate_dir={work}/state\nadmin_token_file={work}/admin-token\njournal_bytes=1073741824\njournal_file_bytes=67108864\nretention_s=86400\nretention_bytes=1073741824\nquery_plan=walk\n')
    ctx=ssl.create_default_context(cafile=str(work/'ca.pem'))
    groups={}
    for name,maximum,high,cores in [('server',384,320,2),('node',64,48,1)]:
        groups[name],_=cgroups.subgroup(parent,f'o8-{mode}-{name}',maximum*2**20,high*2**20,128,cores)
    commands=[];kids=[];relay=None;relay_thread=None
    def spawn(name,args):
        argv=[sys.executable,str(ROOT/'tools/bench/labs/completion/enter_group.py'),str(groups[name]),str(bins/name),*map(str,args)]
        commands.append(argv);dump(out/'commands.json',commands)
        p=subprocess.Popen(argv,stdout=(out/f'{name}.stdout').open('wb'),stderr=(out/f'{name}.stderr').open('wb'))
        kids.append(p);return p
    def api(path,body=None):
        req=urllib.request.Request(f'https://127.0.0.1:{port}'+path,
            data=json.dumps(body).encode() if body is not None else None,
            headers={'authorization':'Bearer '+token,'content-type':'application/json'})
        try:
            with urllib.request.urlopen(req,context=ctx,timeout=5) as response:return json.loads(response.read())
        except urllib.error.HTTPError as error:
            raise RuntimeError(f'API {path} rejected {body}: {error.code} {error.read(4096).decode()}') from error
    def pages(q):
        result=[];query=dict(q)
        for _ in range(1000):
            page=api('/v1/admin/query',query);result.append(page)
            if page['next_page'] is None:return result
            query['page']=page['next_page']
        raise RuntimeError('pagination bound exceeded')
    request_events=[];event_lock=threading.Lock()
    visibility_stop=threading.Event();visibility_thread=None;visibility_errors=[]
    first_seen={};clock_offsets=[]
    class Relay(http.server.BaseHTTPRequestHandler):
        def log_message(self,*args):pass
        def do_GET(self):self.forward()
        def do_POST(self):self.forward()
        def forward(self):
            self.connection.settimeout(10)
            size=int(self.headers.get('Content-Length','0'))
            if not 0<=size<=16*2**20:raise RuntimeError('request byte cap')
            body=self.rfile.read(size) if size else None
            req=urllib.request.Request(f'https://127.0.0.1:{port}'+self.path,data=body,method=self.command,
                headers={k:v for k,v in self.headers.items() if k.lower() not in ('host','connection','content-length')})
            start=time.time_ns()
            try:
                with urllib.request.urlopen(req,context=ctx,timeout=5) as response:
                    status=response.status;payload=response.read(65537)
            except urllib.error.HTTPError as error:status=error.code;payload=error.read(65537)
            if len(payload)>65536:raise RuntimeError('response byte cap')
            committed=time.time_ns()
            if self.path=='/v1/batches':
                time.sleep(.05)
                decoded=query_oracle.decode_batch(body)
                with event_lock:request_events.append({'sequence':decoded['sequence'],'sha256':hashlib.sha256(body).hexdigest(),
                    'request_start_ns':start,'server_answer_ns':committed,'relay_answer_ns':time.time_ns(),'status':status,
                    'response':payload.decode(),'encoded_bytes':size})
            self.send_response(status);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(payload)))
            self.end_headers();self.wfile.write(payload)
    try:
        server=spawn('server',['serve',conf])
        startup=min(deadline,time.monotonic()+10)
        while True:
            try:api('/v1/health');break
            except Exception:
                if server.poll() is not None or time.monotonic()>startup:raise RuntimeError('server unavailable')
                time.sleep(.05)
        logs=work/'input.log';logs.touch()
        node_token=api('/v1/admin/nodes',{'name':'o8-node','logs':[str(logs)],'metric_interval_s':15})['token']
        (work/'node-token').write_text(node_token+'\n')
        relay=http.server.ThreadingHTTPServer(('127.0.0.1',relay_port),Relay);relay.daemon_threads=True
        tls=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);tls.load_cert_chain(work/'server.pem',work/'server.key')
        relay.socket=tls.wrap_socket(relay.socket,server_side=True)
        relay_thread=threading.Thread(target=relay.serve_forever,daemon=True);relay_thread.start()
        host=work/'host';host.mkdir()
        fixtures={'stat':'cpu 10 0 5 20 0 0 0 0 0 0\nbtime 1000\n','meminfo':'MemTotal: 1000 kB\nMemAvailable: 500 kB\n',
            'diskstats':'8 0 sda 1 0 4 0 1 0 8 0\n','netdev':'Inter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast |bytes packets errs drop fifo colls carrier compressed\neth0: 10 0 0 0 0 0 0 0 20 0 0 0 0 0 0 0\n',
            'boot_id':'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa','hostname':'o8-fixture\n'}
        for name,text in fixtures.items():(host/name).write_text(text)
        node_conf=work/'node.conf'
        node_conf.write_text(f'spool_dir={work}/spool\nlog={logs}\nmetric_interval_s=15\nspool_bytes=1048576\nserver_url=https://127.0.0.1:{relay_port}\nserver_ca={work}/ca.pem\ntoken_file={work}/node-token\n')
        node=spawn('node',[node_conf,mode,20,host,work/'stop'])
        def poll_visibility():
            previous=None
            try:
                with gzip.open(out/'visibility-pages.jsonl.gz','wt') as stream:
                    while not visibility_stop.is_set():
                        began_poll=time.time_ns();clock_offsets.append(began_poll-time.monotonic_ns())
                        page=api('/v1/admin/query',{'kind':'logs','from_ns':0,'to_ns':2**64-1,'limit':1000})
                        received=time.time_ns()
                        if not page['complete'] or page['next_page'] is not None:raise RuntimeError('visibility acquisition incomplete/paginated')
                        stream.write(json.dumps({'started_ns':began_poll,'received_ns':received,'answer':page})+'\n')
                        for row in page['rows']:
                            first_seen.setdefault(row['body'],{'lower_ns':previous,'upper_ns':received})
                        previous=began_poll
                        visibility_stop.wait(.1)
            except Exception as error:visibility_errors.append(str(error))
        visibility_thread=threading.Thread(target=poll_visibility,daemon=True);visibility_thread.start()
        offered=[];offering=time.monotonic()
        with logs.open('ab',buffering=0) as source:
            for tick in range(150):
                rate=(10,30,10)[tick//50]
                target=offering+tick/10
                if target>time.monotonic():time.sleep(target-time.monotonic())
                for _ in range(rate//10):
                    index=len(offered);body=f'o8-seed42-{index:06}-'+('R'*(512-17))
                    timestamp=time.time_ns();source.write((body+'\n').encode())
                    offered.append({'body':body,'offered_ns':timestamp})
                if time.monotonic()>deadline:raise RuntimeError('native deadline exhausted')
        node.wait(timeout=min(30,max(.1,deadline-time.monotonic())))
        if node.returncode:raise RuntimeError('node failed')
        visibility_stop.set();visibility_thread.join(timeout=10)
        if visibility_thread.is_alive() or visibility_errors:raise RuntimeError('visibility poll failed: '+repr(visibility_errors))
        if clock_offsets and max(clock_offsets)-min(clock_offsets)>10_000_000:raise RuntimeError('wall/monotonic offset moved over10ms')
        queries=[{'kind':'logs','from_ns':0,'to_ns':2**64-1,'limit':50},
            {'kind':'metrics','name':'system.network.receive.bytes','from_ns':0,'to_ns':2**64-1,'limit':50},
            {'kind':'rate','name':'system.network.receive.bytes','from_ns':0,'to_ns':2**64-1}]
        answers=[pages(q) for q in queries]
        server.send_signal(signal.SIGTERM);server.wait(timeout=10)
        if server.returncode:raise RuntimeError('server failed graceful exit')
        with (work/'recovered.jsonl').open('wb') as dst:
            subprocess.run([str(bins/'dump'),str(conf),'--records'],stdout=dst,stderr=(out/'dump.stderr').open('wb'),check=True,
                timeout=min(30,max(.1,deadline-time.monotonic())))
        recovered=[];hashes={};observed=[]
        for line in (work/'recovered.jsonl').read_text().splitlines():
            entry=json.loads(line);raw=base64.b64decode(entry['bytes']);batch=query_oracle.decode_batch(raw)
            recovered.append(entry);hashes[batch['sequence']]=hashlib.sha256(raw).hexdigest()
            observed.extend(log['body'] for log in query_oracle.decode_logs_request(batch['logs_bytes']))
        if observed!=[o['body'] for o in offered]:raise RuntimeError('offered/committed logs differ')
        producer=[json.loads(line) for line in (work/'producer.jsonl').read_text().splitlines()]
        if len(producer)!=len(recovered) or len(producer)!=len(hashes) or sorted(hashes)!=list(range(1,len(hashes)+1)):
            raise RuntimeError('missing/duplicate/noncontiguous committed Batch sequence')
        if {p['sequence']:hashlib.sha256(bytes.fromhex(p['hex'])).hexdigest() for p in producer}!=hashes:raise RuntimeError('Spool/server custody drift')
        identities={(query_oracle.decode_batch(bytes.fromhex(p['hex']))['node_id'],
            query_oracle.decode_batch(bytes.fromhex(p['hex']))['generation']) for p in producer}
        if len(identities)!=1:raise RuntimeError('fixture is not one Strand')
        verdicts=[query_oracle.check(recovered,q,a) for q,a in zip(queries,answers)]
        if not all(v['passed'] for v in verdicts):raise RuntimeError('complete query oracle failed')
        for event in request_events:
            if hashes[event['sequence']]!=event['sha256'] or event['status']!=200:raise RuntimeError('request/ACK drift')
        events=[json.loads(line) for line in (out/'node.stdout').read_text().splitlines()]
        commits={e['sequence']:e['unix_ns'] for e in events if e['event']=='commit'}
        acked={e['sequence']:e['unix_ns'] for e in events if e['event']=='attempt' and e['outcome']==f"Ack({e['sequence']})"}
        if set(acked)!=set(hashes) or set(commits)!=set(hashes):raise RuntimeError('ACK/commit sourceassociation missing')
        if len(request_events)!=len(producer):raise RuntimeError('unexpected normal-trial retries')
        spool_ack=[(acked[s]-t)/1e6 for s,t in commits.items()]
        cycle=[e['elapsed_ns']/1e6 for e in events if e['event']=='cycle']
        body_ack={};body_commit={}
        for p in producer:
            for log in query_oracle.decode_logs_request(query_oracle.decode_batch(bytes.fromhex(p['hex']))['logs_bytes']):
                body_ack[log['body']]=acked[p['sequence']];body_commit[log['body']]=commits[p['sequence']]
        if set(first_seen)!={o['body'] for o in offered}:raise RuntimeError('visibility missed/unexpected sourcebody')
        if any(body_commit[o['body']]<o['offered_ns'] for o in offered):raise RuntimeError('Spool commit before sourceoffer')
        observation_ack=[(body_ack[o['body']]-o['offered_ns'])/1e6 for o in offered]
        observation_spool=[(body_commit[o['body']]-o['offered_ns'])/1e6 for o in offered]
        ack_visibility=[(first_seen[o['body']]['upper_ns']-body_ack[o['body']])/1e6 for o in offered]
        negative={}
        import copy
        for mutation in ('missing','duplicate'):
            bad=copy.deepcopy(answers[0])
            if mutation=='missing':bad[0]['rows'].pop()
            else:bad[0]['rows'].append(copy.deepcopy(bad[0]['rows'][0]))
            rejected=query_oracle.check(recovered,queries[0],bad)
            if rejected['passed']:raise RuntimeError('queryoracle accepted '+mutation)
            negative[mutation]=rejected
        mismatch=dict(hashes);mismatch[producer[0]['sequence']]='MUTATION'
        negative['changed_custody_rejected']=mismatch!={p['sequence']:hashlib.sha256(bytes.fromhex(p['hex'])).hexdigest() for p in producer}
        cpu_stat=dict(line.split() for line in (groups['node']/'cpu.stat').read_text().splitlines())
        cpu_s=int(cpu_stat['usage_usec'])/1e6
        logical_bytes=sum(len(o['body'].encode()) for o in offered)
        finished=next(e for e in events if e['event']=='finished')
        if not finished['caught_up'] or finished['acked']!=finished['last']:raise RuntimeError('unacknowledged final custody')
        result={'mode':mode,'offered_logs':len(offered),'committed_batches':len(producer),'acked_batches':len(acked),'encoded_batch_bytes':sum(len(bytes.fromhex(p['hex'])) for p in producer),
            'source_logical_bytes':logical_bytes,'node_cpu_seconds':cpu_s,'node_cpu_seconds_per_logical_mib':cpu_s/(logical_bytes/2**20),
            'node_wall_seconds':finished['wall_ns']/1e9,'query_verdicts':verdicts,
            'spool_to_ack':summary(spool_ack),'observation_to_ack':summary(observation_ack),'observation_to_spool':summary(observation_spool),
            'ack_to_queryable_upper_bound':summary(ack_visibility),'cycle':summary(cycle),'negative_controls':negative,
            'wall_monotonic_offset_range_ns':max(clock_offsets)-min(clock_offsets) if clock_offsets else None,'fixed_ack_relay_delay_ms':50,
            'no_p99_or_capacity_claim':True,'service_cgroups':cgroups.snapshot(parent)}
        for name,value in [('source.json',offered),('requests.json',request_events),('query-pages.json',answers),('queries.json',queries),('visibility-first-seen.json',first_seen),('result.json',result)]:dump(out/name,value)
        for name in ('producer.jsonl','recovered.jsonl'):
            with (work/name).open('rb') as src,gzip.open(out/(name+'.gz'),'wb') as dst:shutil.copyfileobj(src,dst)
        return result
    finally:
        visibility_stop.set()
        if visibility_thread:visibility_thread.join(timeout=10)
        for p in kids:
            if p.poll() is None:
                p.terminate()
                try:p.wait(timeout=10)
                except subprocess.TimeoutExpired:p.kill();p.wait(timeout=5)
        if relay is not None:relay.shutdown();relay.server_close()
        if relay_thread:relay_thread.join(timeout=2)

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--stage',choices=('freeze','pair'),required=True);parser.add_argument('--out',type=Path,required=True)
    parser.add_argument('--freeze',type=Path);parser.add_argument('--seconds',type=int,default=300)
    args=parser.parse_args();require_limits()
    if not 1<=args.seconds<=900:parser.error('seconds must be1..900')
    if args.stage=='pair' and args.freeze is None:parser.error('pair requires --freeze')
    out=args.out.resolve();scratch=Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    if out.exists() or not out.is_relative_to(ROOT/'docs/experiments/benchmarks/data') or not out.name.startswith('catalog-overlap-') or not scratch.is_relative_to(STORAGE/'scratch'):raise RuntimeError('owned fresh evidence/mounted scratch required')
    out.mkdir();work=scratch/'o8-overlap';work.mkdir();deadline=time.monotonic()+args.seconds;complete=False
    try:
        if args.stage=='freeze':freeze(out,work,deadline)
        else:
            receipt=json.loads((args.freeze/'freeze.json').read_text());bins=work/'bins';bins.mkdir()
            if receipt['status']!='complete':raise RuntimeError('incomplete freeze')
            for name,item in receipt['binaries'].items():
                archive=args.freeze/item['archive'];path=bins/name
                if sha(archive)!=item['archive_sha256']:raise RuntimeError('archive changed')
                with gzip.open(archive,'rb') as src,path.open('xb') as dst:shutil.copyfileobj(src,dst)
                if sha(path)!=item['decoded_sha256'] or path.stat().st_size!=item['decoded_bytes']:raise RuntimeError('decoded executable changed')
                path.chmod(0o700)
            parent=cgroups.delegate();results=[]
            for mode in ('serial','overlap'):
                results.append(trial(work/mode,out/mode,mode,bins,parent,deadline));dump(out/'results.json',results)
            dump(out/'freeze-reference.json',receipt)
            dump(out/'execution-driver.json',{'path':str(Path(__file__).relative_to(ROOT)),'sha256':sha(Path(__file__))})
            baseline,candidate=results
            dump(out/'comparison.json',{'single_pair_screen_only':True,
                'observation_ack_median_ratio':candidate['observation_to_ack']['median_ms']/baseline['observation_to_ack']['median_ms'],
                'node_cpu_per_logical_mib_ratio':candidate['node_cpu_seconds_per_logical_mib']/baseline['node_cpu_seconds_per_logical_mib'],
                'both_exact_and_fully_acked':True,'no_p99_capacity_or_nomination_claim':True})
        files={}
        for folder in out.parent.glob('catalog-overlap-*'):
            for path in folder.rglob('*'):
                if path.is_file() and not path.is_symlink():
                    stat=path.stat();files[(stat.st_dev,stat.st_ino)]=(stat.st_size,stat.st_blocks*512)
        usage={'unique_inode_bytes':sum(v[0] for v in files.values()),'allocated_bytes':sum(v[1] for v in files.values())}
        dump(out/'evidence-accounting.json',usage)
        if max(usage.values())>32*2**20:raise RuntimeError('32MiB aggregate pilot evidence exhausted')
        complete=True
    finally:
        dump(out/'cleanup.json',{'complete':complete,'removed':complete,'scratch':str(work)})
        if complete:shutil.rmtree(work)

if __name__=='__main__':main()
