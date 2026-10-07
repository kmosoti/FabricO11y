#!/usr/bin/env python3
"""Separately registered O8 20-node fixed-demand screen; frozen native binaries."""
import argparse
import base64
import copy
import gzip
import hashlib
import http.server
import json
import os
from pathlib import Path
import shutil
import signal
import ssl
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
import coupled_overlap as common

ROOT=common.ROOT

def save(path,value):
    with gzip.open(path,'wt') as stream:stream.write(json.dumps(value,indent=2)+'\n')

def identity(raw):
    batch=common.query_oracle.decode_batch(raw)
    return (batch['node_id'].hex(),batch['generation'],batch['sequence'])

def body_of(node,index):
    prefix=f'o8-small42-{node:02}-{index:06}-'
    return prefix+'R'*(512-len(prefix))

def arm(work,out,mode,bins,parent,deadline):
    work.mkdir();out.mkdir();common.make_certs(work)
    port,relay_port=common.free_port(),common.free_port()
    admin='o8-small-admin-0123456789abcdef0123456789abcdef'
    (work/'admin-token').write_text(admin+'\n')
    conf=work/'server.conf'
    conf.write_text(f'listen=127.0.0.1:{port}\ntls_cert={work}/server.pem\ntls_key={work}/server.key\nstate_dir={work}/state\nadmin_token_file={work}/admin-token\njournal_bytes=1073741824\njournal_file_bytes=67108864\nretention_s=86400\nretention_bytes=1073741824\nquery_plan=walk\n')
    ctx=ssl.create_default_context(cafile=str(work/'ca.pem'))
    host,host_limits=common.cgroups.subgroup(parent,f'o8-small-{mode}-host',3328*2**20,2816*2**20,640,4,True)
    groups={};limits={'host':host_limits}
    groups['server'],limits['server']=common.cgroups.subgroup(host,'server',3072*2**20,2560*2**20,512,2)
    for index in range(20):
        name=f'node{index:02}'
        groups[name],limits[name]=common.cgroups.subgroup(host if index==0 else parent,
            f'o8-small-{mode}-{name}',256*2**20,192*2**20,128,1)
    common.dump(out/'limits.json',limits)
    kids={};commands=[];relay=None;relay_thread=None;observer=None
    stop=threading.Event();errors=[];requests=[];request_lock=threading.Lock()
    seen={};polls=[];resources=[];anchors=[]
    def spawn(name,binary,args):
        argv=[sys.executable,str(ROOT/'tools/bench/labs/completion/enter_group.py'),str(groups[name]),str(bins/binary),*map(str,args)]
        commands.append(argv);common.dump(out/'commands.json',commands)
        child=subprocess.Popen(argv,stdout=(out/f'{name}.stdout').open('wb'),stderr=(out/f'{name}.stderr').open('wb'))
        kids[name]=child;return child
    def api(path,body=None):
        req=urllib.request.Request(f'https://127.0.0.1:{port}'+path,
            data=json.dumps(body).encode() if body is not None else None,
            headers={'authorization':'Bearer '+admin,'content-type':'application/json'})
        try:
            with urllib.request.urlopen(req,context=ctx,timeout=5) as answer:return json.loads(answer.read())
        except urllib.error.HTTPError as error:
            raise RuntimeError(f'API {path}: {error.code} {error.read(4096).decode()}') from error
    def pages(query):
        answers=[];q=dict(query)
        for _ in range(1000):
            if time.monotonic()>deadline:raise RuntimeError('query deadline')
            answer=api('/v1/admin/query',q);answers.append(answer)
            if answer['next_page'] is None:return answers
            q['page']=answer['next_page']
        raise RuntimeError('pagination bound')
    class Relay(http.server.BaseHTTPRequestHandler):
        def log_message(self,*args):pass
        def do_GET(self):self.forward()
        def do_POST(self):self.forward()
        def forward(self):
            self.connection.settimeout(10)
            size=int(self.headers.get('Content-Length','0'))
            if not 0<=size<=16*2**20:raise RuntimeError('request cap')
            data=self.rfile.read(size) if size else None
            req=urllib.request.Request(f'https://127.0.0.1:{port}'+self.path,data=data,method=self.command,
                headers={k:v for k,v in self.headers.items() if k.lower() not in ('host','connection','content-length')})
            began=time.time_ns()
            try:
                with urllib.request.urlopen(req,context=ctx,timeout=5) as answer:
                    status=answer.status;payload=answer.read(65537)
            except urllib.error.HTTPError as error:status=error.code;payload=error.read(65537)
            if len(payload)>65536:raise RuntimeError('response cap')
            answered=time.time_ns()
            if self.path=='/v1/batches':
                time.sleep(.05)
                with request_lock:requests.append({'identity':identity(data),'sha256':hashlib.sha256(data).hexdigest(),
                    'status':status,'encoded_bytes':size,'request_start_ns':began,'server_answer_ns':answered,
                    'relay_answer_ns':time.time_ns(),'response':payload.decode()})
            self.send_response(status);self.send_header('Content-Type','application/json')
            self.send_header('Content-Length',str(len(payload)));self.end_headers();self.wfile.write(payload)
    try:
        server=spawn('server','server',['serve',conf]);startup=min(deadline,time.monotonic()+10)
        while True:
            try:api('/v1/health');break
            except Exception:
                if server.poll() is not None or time.monotonic()>startup:raise RuntimeError('server startup failed')
                time.sleep(.05)
        class SmallRelayServer(http.server.ThreadingHTTPServer):
            request_queue_size=128
        relay=SmallRelayServer(('127.0.0.1',relay_port),Relay);relay.daemon_threads=True
        tls=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);tls.load_cert_chain(work/'server.pem',work/'server.key')
        relay.socket=tls.wrap_socket(relay.socket,server_side=True)
        relay_thread=threading.Thread(target=relay.serve_forever,daemon=True);relay_thread.start()
        node_dirs=[];logs=[]
        fixtures={'stat':'cpu 10 0 5 20 0 0 0 0 0 0\nbtime 1000\n','meminfo':'MemTotal: 1000 kB\nMemAvailable: 500 kB\n',
            'diskstats':'8 0 sda 1 0 4 0 1 0 8 0\n','netdev':'Inter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast |bytes packets errs drop fifo colls carrier compressed\neth0: 10 0 0 0 0 0 0 0 20 0 0 0 0 0 0 0\n',
            'boot_id':'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa','hostname':'o8-small-fixture\n'}
        for index in range(20):
            name=f'node{index:02}';directory=work/name;directory.mkdir();node_dirs.append(directory)
            log=directory/'input.log';log.touch();logs.append(log)
            token=api('/v1/admin/nodes',{'name':name,'logs':[str(log)],'metric_interval_s':15})['token']
            (directory/'token').write_text(token+'\n');host_files=directory/'host';host_files.mkdir()
            for key,text in fixtures.items():(host_files/key).write_text(text)
            config=directory/'node.conf'
            config.write_text(f'spool_dir={directory}/spool\nlog={log}\nmetric_interval_s=15\nspool_bytes=8388608\nserver_url=https://127.0.0.1:{relay_port}\nserver_ca={work}/ca.pem\ntoken_file={directory}/token\n')
        for index,directory in enumerate(node_dirs):spawn(f'node{index:02}','node',[directory/'node.conf',mode,20,directory/'host',directory/'stop'])
        sentinels=[(index,body_of(index,offset)) for index in range(20) for offset in (0,250,1000)]
        def observe():
            cursor=0;previous={};next_resource=0
            try:
                with gzip.open(out/'sentinel-pages.jsonl.gz','wt') as stream,(out/'resources.jsonl').open('w') as resource_stream:
                    while not stop.is_set():
                        index,body=sentinels[cursor%60];cursor+=1
                        began=time.time_ns();anchors.append(began-time.monotonic_ns())
                        query={'kind':'logs','node':f'node{index:02}','contains':body,'from_ns':0,'to_ns':2**64-1,'limit':1}
                        answer=api('/v1/admin/query',query);received=time.time_ns()
                        if not answer['complete'] or answer['next_page'] is not None:raise RuntimeError('sentinel query incomplete')
                        if any(row['body']!=body for row in answer['rows']):raise RuntimeError('sentinel filter mismatch')
                        stream.write(json.dumps({'started_ns':began,'received_ns':received,'query':query,'answer':answer})+'\n');stream.flush()
                        if answer['rows']:seen.setdefault(body,{'lower_ns':previous.get(body),'upper_ns':received})
                        previous[body]=began
                        if time.monotonic()>=next_resource:
                            snapshot={'unix_ns':time.time_ns(),'services':{}}
                            for name,child in kids.items():
                                values={key:(groups[name]/key).read_text().strip() for key in ('cpu.stat','io.stat','memory.current','memory.peak','memory.events')}
                                if name!='server':
                                    spool_bytes=0
                                    for path in (work/name/'spool').rglob('*'):
                                        try:
                                            if path.is_file():spool_bytes+=path.stat().st_size
                                        except FileNotFoundError:pass
                                    values['sampled_spool_file_bytes']=spool_bytes
                                try:
                                    for line in Path(f'/proc/{child.pid}/status').read_text().splitlines():
                                        if line.startswith(('VmRSS:','VmHWM:')):
                                            key,value,unit=line.split()
                                            if unit!='kB':raise RuntimeError('RSS unit')
                                            values[key[:-1]]=int(value)*1024
                                except FileNotFoundError:pass
                                snapshot['services'][name]=values
                            resources.append(snapshot);resource_stream.write(json.dumps(snapshot)+'\n');resource_stream.flush()
                            next_resource=time.monotonic()+1
                        stop.wait(.05)
            except Exception as error:errors.append(str(error))
        observer=threading.Thread(target=observe,daemon=True);observer.start()
        offered=[];sources=[path.open('ab',buffering=0) for path in logs];counts=[0]*20;began=time.monotonic()
        try:
            with gzip.open(out/'source.jsonl.gz','wt') as source_evidence:
                for tick in range(150):
                    target=began+tick/10
                    if target>time.monotonic():time.sleep(target-time.monotonic())
                    for index,source in enumerate(sources):
                        for _ in range((50,150,50)[tick//50]//10):
                            body=body_of(index,counts[index]);counts[index]+=1;timestamp=time.time_ns()
                            source.write((body+'\n').encode());entry={'node':f'node{index:02}','body':body,'offered_ns':timestamp}
                            offered.append(entry);source_evidence.write(json.dumps(entry)+'\n')
                    if time.monotonic()>deadline:raise RuntimeError('offer deadline')
        finally:
            for source in sources:source.close()
        if counts!=[1250]*20:raise RuntimeError('offer count drift')
        for name,child in kids.items():
            if name!='server':
                child.wait(timeout=min(30,max(.1,deadline-time.monotonic())))
                if child.returncode:raise RuntimeError(name+' failed')
        stop.set();observer.join(timeout=10)
        if observer.is_alive() or errors:raise RuntimeError('observer failed: '+repr(errors))
        if set(seen)!={body for _,body in sentinels}:raise RuntimeError('missing sentinel visibility')
        if max(anchors)-min(anchors)>10_000_000:raise RuntimeError('clock drift')
        queries=[{'kind':'logs','from_ns':0,'to_ns':2**64-1,'limit':1000},
            {'kind':'metrics','name':'system.network.receive.bytes','from_ns':0,'to_ns':2**64-1,'limit':1000},
            {'kind':'rate','name':'system.network.receive.bytes','from_ns':0,'to_ns':2**64-1}]
        answers=[]
        for index,query in enumerate(queries):
            answer=pages(query);answers.append(answer);save(out/f'query-{index}.json.gz',{'query':query,'pages':answer})
        server.send_signal(signal.SIGTERM);server.wait(timeout=10)
        if server.returncode:raise RuntimeError('server shutdown failed')
        with (work/'recovered.jsonl').open('wb') as dst,(out/'dump.stderr').open('wb') as err:
            subprocess.run([str(bins/'dump'),str(conf),'--records'],stdout=dst,stderr=err,check=True,timeout=min(30,max(.1,deadline-time.monotonic())))
        recovered=[json.loads(line) for line in (work/'recovered.jsonl').read_text().splitlines()]
        hashes={};node_hashes={};actual_bodies={};body_ack={};body_commit={};overlapped=[];transitions=0;spool_ack=[];cpu=0;finished=[]
        for record in recovered:
            raw=base64.b64decode(record['bytes']);key=identity(raw)
            if key in hashes:raise RuntimeError('duplicate server Batch')
            hashes[key]=hashlib.sha256(raw).hexdigest()
            for log in common.query_oracle.decode_logs_request(common.query_oracle.decode_batch(raw)['logs_bytes']):
                if log['body'] in actual_bodies:raise RuntimeError('duplicate recovered log')
                actual_bodies[log['body']]=record['label']
        for index,directory in enumerate(node_dirs):
            name=f'node{index:02}';events=[json.loads(line) for line in (out/f'{name}.stdout').read_text().splitlines()]
            commits={e['sequence']:e['unix_ns'] for e in events if e['event']=='commit'}
            acks={e['sequence']:e['unix_ns'] for e in events if e['event']=='attempt' and e['outcome']==f"Ack({e['sequence']})"}
            producer=[json.loads(line) for line in (directory/'producer.jsonl').read_text().splitlines()]
            sequences=[p['sequence'] for p in producer]
            if sequences!=list(range(1,len(producer)+1)) or set(commits)!=set(sequences) or set(acks)!=set(sequences):raise RuntimeError('node sequence/ACK/commit mismatch')
            strands=set()
            for record in producer:
                raw=bytes.fromhex(record['hex']);key=identity(raw);strands.add(key[:2])
                if key in node_hashes:raise RuntimeError('duplicate producer identity')
                node_hashes[key]=hashlib.sha256(raw).hexdigest()
                sequence=key[2];spool_ack.append((acks[sequence]-commits[sequence])/1e6)
                for log in common.query_oracle.decode_logs_request(common.query_oracle.decode_batch(raw)['logs_bytes']):
                    body_ack[log['body']]=acks[sequence];body_commit[log['body']]=commits[sequence]
            if len(strands)!=1:raise RuntimeError('node Strand mismatch')
            for sequence in commits:
                if sequence>1:
                    transitions+=1
                    if commits[sequence]<acks[sequence-1]:overlapped.append({'node':name,'sequence':sequence})
            final=next(e for e in events if e['event']=='finished');finished.append(final)
            if not final['caught_up'] or final['acked']!=final['last']:raise RuntimeError('undrained node')
            cpu+=int(dict(line.split() for line in (groups[name]/'cpu.stat').read_text().splitlines())['usage_usec'])/1e6
            with (directory/'producer.jsonl').open('rb') as src,gzip.open(out/f'{name}-producer.jsonl.gz','wb') as dst:shutil.copyfileobj(src,dst)
        expected_bodies={entry['body']:entry['node'] for entry in offered}
        if actual_bodies!=expected_bodies or hashes!=node_hashes or len({key[:2] for key in hashes})!=20:raise RuntimeError('exact source/custody mismatch')
        if len(requests)!=len(hashes) or any(tuple(e['identity']) not in hashes or e['sha256']!=hashes[tuple(e['identity'])] or e['status']!=200 for e in requests):raise RuntimeError('normal request/retry/custody mismatch')
        verdicts=[common.query_oracle.check(recovered,q,a) for q,a in zip(queries,answers)]
        if not all(v['passed'] for v in verdicts):raise RuntimeError('full query oracle failed')
        negative={}
        for mutation in ('missing','duplicate'):
            altered=copy.deepcopy(answers[0])
            if mutation=='missing':altered[0]['rows'].pop()
            else:altered[0]['rows'].append(copy.deepcopy(altered[0]['rows'][0]))
            verdict=common.query_oracle.check(recovered,queries[0],altered)
            if verdict['passed']:raise RuntimeError('negative query accepted')
            negative[mutation]=verdict
        altered=dict(node_hashes);altered[next(iter(altered))]='MUTATION'
        if altered==hashes:raise RuntimeError('negative custody accepted')
        negative['changed_custody_rejected']=True
        if any(body_commit[e['body']]<e['offered_ns'] for e in offered):raise RuntimeError('commit before offer')
        logical_bytes=25000*512
        result={'mode':mode,'node_count':20,'offered_logs':25000,'accepted_logical_bytes':logical_bytes,
            'accepted_batches':len(hashes),'accepted_encoded_bytes':sum(e['encoded_bytes'] for e in requests),
            'acked_batches':len(hashes),'normal_retries':0,'server_request_refusals':0,
            'collection_errors':[{'node':name,'event':e} for name in kids if name!='server' for e in map(json.loads,(out/f'{name}.stdout').read_text().splitlines()) if e['event']=='collection_error'],
            'byte_refusal_measurement':'no separate byte-refusal instrumentation',
            'node_cpu_seconds':cpu,'node_cpu_per_logical_mib':cpu/(logical_bytes/2**20),
            'observation_to_ack':common.summary([(body_ack[e['body']]-e['offered_ns'])/1e6 for e in offered]),
            'observation_to_spool':common.summary([(body_commit[e['body']]-e['offered_ns'])/1e6 for e in offered]),
            'spool_to_ack':common.summary(spool_ack),'eligible_transitions':transitions,'prepared_before_previous_ack':overlapped,
            'sentinel_visibility_samples':len(seen),'sentinel_ack_to_visible_upper':common.summary([(seen[b]['upper_ns']-body_ack[b])/1e6 for b in seen]),
            'query_verdicts':verdicts,'negative_controls':negative,'service_cgroups':common.cgroups.snapshot(parent),
            'final_io_stat':{name:(group/'io.stat').read_text().strip() for name,group in groups.items()},
            'sampled_rss_peak_bytes':{name:max((sample['services'][name].get('VmRSS',0) for sample in resources),default=0) for name in kids},
            'sampled_spool_peak_file_bytes':{name:max((sample['services'][name].get('sampled_spool_file_bytes',0) for sample in resources),default=0) for name in kids if name!='server'},
            'clock_offset_range_ns':max(anchors)-min(anchors),'no_capacity_p99_or_nomination_claim':True}
        common.dump(out/'result.json',result);save(out/'requests.json.gz',requests);save(out/'sentinel-first-seen.json.gz',seen)
        with (work/'recovered.jsonl').open('rb') as src,gzip.open(out/'recovered.jsonl.gz','wb') as dst:shutil.copyfileobj(src,dst)
        return result
    finally:
        stop.set()
        if observer:observer.join(timeout=10)
        for child in kids.values():
            if child.poll() is None:child.terminate()
        cleanup_end=time.monotonic()+10
        for child in kids.values():
            if child.poll() is None:
                try:child.wait(timeout=max(.1,cleanup_end-time.monotonic()))
                except subprocess.TimeoutExpired:child.kill()
        for child in kids.values():
            if child.poll() is None:child.wait(timeout=2)
        if relay is not None:relay.shutdown();relay.server_close()
        if relay_thread:relay_thread.join(timeout=2)

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--freeze',type=Path,required=True)
    parser.add_argument('--out',type=Path,required=True);parser.add_argument('--seconds',type=int,default=170)
    args=parser.parse_args();common.require_limits()
    if not 1<=args.seconds<=170:parser.error('seconds1..170')
    out=args.out.resolve();scratch=Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    if out.exists() or not out.is_relative_to(ROOT/'docs/experiments/benchmarks/data') or not out.name.startswith('catalog-overlap-small-') or not scratch.is_relative_to(common.STORAGE/'scratch'):raise RuntimeError('fresh owned evidence/mounted scratch required')
    out.mkdir();work=scratch/'o8-small';work.mkdir();complete=False;deadline=time.monotonic()+args.seconds
    common.dump(out/'execution.json',{'argv':sys.argv,'driver_sha256':common.sha(Path(__file__)),
        'helper_sha256':common.sha(Path(common.__file__)),'profile':'small','seed':42,'node_count':20})
    try:
        receipt=json.loads((args.freeze/'freeze.json').read_text());bins=work/'bins';bins.mkdir()
        if receipt['status']!='complete':raise RuntimeError('incomplete freeze')
        for name,item in receipt['binaries'].items():
            archive=args.freeze/item['archive'];path=bins/name
            if common.sha(archive)!=item['archive_sha256']:raise RuntimeError('archive changed')
            with gzip.open(archive,'rb') as src,path.open('xb') as dst:shutil.copyfileobj(src,dst)
            if common.sha(path)!=item['decoded_sha256'] or path.stat().st_size!=item['decoded_bytes']:raise RuntimeError('binary changed')
            path.chmod(0o700)
        common.dump(out/'freeze-reference.json',receipt);parent=common.cgroups.delegate();results=[]
        for mode in ('serial','overlap'):
            results.append(arm(work/mode,out/mode,mode,bins,parent,deadline));common.dump(out/'results.json',results)
        baseline,candidate=results
        common.dump(out/'comparison.json',{'single_pair_screen_only':True,'exact_and_fully_acked':True,
            'source_ack_median_ratio':candidate['observation_to_ack']['median_ms']/baseline['observation_to_ack']['median_ms'],
            'node_cpu_per_logical_mib_ratio':candidate['node_cpu_per_logical_mib']/baseline['node_cpu_per_logical_mib'],
            'mechanism_status':'exposed' if candidate['prepared_before_previous_ack'] else 'inconclusive_no_prepared_successor',
            'no_capacity_p99_or_nomination_claim':True})
        size=sum(path.stat().st_size for path in out.rglob('*') if path.is_file())
        allocated=sum(path.stat().st_blocks*512 for path in out.rglob('*') if path.is_file())
        common.dump(out/'evidence-accounting.json',{'file_bytes':size,'allocated_bytes':allocated})
        if max(size,allocated)>16*2**20:raise RuntimeError('16MiB small screen evidence exhausted')
        complete=True
    finally:
        common.dump(out/'cleanup.json',{'complete':complete,'removed':complete,'scratch':str(work)})
        if complete:shutil.rmtree(work)

if __name__=='__main__':main()
