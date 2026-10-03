#!/usr/bin/env python3
"""Registered medium/enterprise server simulations and recovered worker control."""
import argparse
import base64
from collections import defaultdict
import gzip
import hashlib
import json
import math
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import threading
import time
import shutil

REPO=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(REPO/'tools/bench'))
from run_dev_small import dump, process_stats, footprint, percentile
sys.path.insert(0,str(REPO/'tools/qualification'))
from delivery_faults import make_certs,free_port
import query_oracle, delivery_oracle


def weighted(values):
    a=sorted(values); total=sum(n for _,n in a)
    def at(q):
        count=0
        for value,n in a:
            count+=n
            if count>=math.ceil(q*total): return value
        return None
    return {'samples':total,'p50':at(.5),'p99':at(.99),'max':a[-1][0] if a else None,'negative':sum(n for value,n in a if value<0)}


def trial(root,bins,name,identities,workers,cpus):
    root.mkdir();(root/'owned').write_text('scaled fleet pilot\n');make_certs(root)
    subprocess.run([str(bins/'examples/ingest_load'),'enroll',str(root/'state'),str(root/'tokens'),str(identities)],check=True,timeout=120)
    (root/'all-tokens').write_text(''.join((root/'tokens'/f'token-{n:04}').read_text() for n in range(identities)))
    (root/'admin').write_text(os.urandom(32).hex()+'\n');port=free_port()
    conf=root/'server.conf';conf.write_text(f'listen=127.0.0.1:{port}\ntls_cert={root}/server.pem\ntls_key={root}/server.key\nstate_dir={root}/state\nadmin_token_file={root}/admin\njournal_bytes=1073741824\njournal_file_bytes=67108864\nretention_s=86400\nretention_bytes=8589934592\nseal_workers={workers}\n')
    children=[];samples=[];errors=[];stop=threading.Event();began=time.monotonic()
    def spawn(binary,args,affinity,limit,label):
        p=subprocess.Popen(['prlimit',f'--as={limit}','--','taskset','-c',','.join(map(str,affinity)),str(binary),*map(str,args)],stdout=open(root/(label+'.out'),'wb'),stderr=open(root/(label+'.err'),'wb'))
        children.append(p);return p
    try:
        server=spawn(bins/'fabric-server',['serve',conf],cpus[:2],4*2**30,'server')
        for _ in range(200):
            if server.poll() is not None:raise RuntimeError('server startup exit')
            try:
                with socket.create_connection(('127.0.0.1',port),timeout=.1):break
            except OSError:time.sleep(.05)
        else:raise RuntimeError('startup timeout')
        sim=spawn(bins/'examples/spindle_sim',['--server-url',f'https://127.0.0.1:{port}','--ca',root/'ca.pem','--tokens',root/'all-tokens','--seed','0xA11FA001','--seconds','25','--workers','128','--out',root/'sim','--burst-from','10','--burst-to','15','--burst-factor','3','--log-factor','25','--body-bytes','900'],cpus[2:4],6*2**30,'sim')
        def sample():
            while not stop.is_set():
                try:
                    if time.monotonic()-began>900:raise RuntimeError('900 s trial bound')
                    size=footprint(root)
                    if size>12*2**30 or shutil.disk_usage(root).free<4*2**30:raise RuntimeError('disk bound')
                    if server.poll() is not None:raise RuntimeError('premature server exit')
                    wall=time.time_ns();mono=time.monotonic_ns()
                    samples.append({'wall_ns':wall,'mono_ns':mono,'server':process_stats(server.pid),'simulator':process_stats(sim.pid) if sim.poll() is None else None,'logical_disk_bytes':size,'sealed_files':len(list((root/'state/journal').glob('sealed-*.faj'))),'segments':len(list((root/'state/segments').glob('seg-*'))),'cgroup_memory_current':int(Path('/sys/fs/cgroup/memory.current').read_text())})
                except Exception as e:errors.append(str(e));stop.set();return
                stop.wait(1)
        sampler=threading.Thread(target=sample);sampler.start()
        while sim.poll() is None:
            if stop.wait(.5):raise RuntimeError('; '.join(errors))
        if sim.returncode:raise RuntimeError('simulator nonzero exit')
        stop.wait(30)
        if errors:raise RuntimeError('; '.join(errors))
        stop.set();sampler.join(5);server.send_signal(signal.SIGTERM);server.wait(timeout=30)
        dump(root/'resources.json',samples)
        if server.returncode:raise RuntimeError('server nonzero exit')
        sim_summary=json.loads((root/'sim/sim-summary.json').read_text());epoch=sim_summary['began_unix_ns']
        creates={};acks={};rtt=[];retry=0;creation_lag=[];generated=[]
        for line in (root/'sim/events.jsonl').open():
            e=json.loads(line)
            if e['e']=='created':
                key=(e['id'],e['seq']);creates[key]=e['t'];creation_lag.append((e['t']-(epoch+(e['seq']-1)*10**9))/1e6);generated.append(e.get('generated_ns',e['t']))
            elif e['e']=='attempt':
                if e['kind']=='ack':acks[(e['id'],e['seq'])]=e['end'];rtt.append((e['end']-e['start'])/1e6)
                else:retry+=1
        # Stream custody, then independently decode log counts/timestamps.
        with (root/'replay.jsonl').open('wb') as out:
            subprocess.run([str(bins/'examples/server_dump'),str(conf),'--records'],stdout=out,stderr=open(root/'dump.err','wb'),check=True,timeout=300)
        total_logs=0;total_bytes=0;bad_sizes=0;populations=defaultdict(list);recovered=0
        with (root/'sim/transcript.jsonl').open('a') as transcript,(root/'replay.jsonl').open() as replay,gzip.open(root/'batch-clocks.jsonl.gz','wt') as clocks:
            for line in replay:
                r=json.loads(line);raw=base64.b64decode(r['bytes']);b=query_oracle.decode_batch(raw);logs=query_oracle.decode_logs_request(b['logs_bytes']);n=len(logs);i=int(r['label'].split('-')[-1]);seq=b['sequence'];key=(i,seq);scheduled=epoch+(seq-1)*10**9;ack=acks.get(key);observed=creates.get(key)
                recovered+=1;total_logs+=n;total_bytes+=len(raw);bad_sizes+=sum(len(x['body'].encode())!=900 for x in logs)
                phase='normal' if seq<=10 else 'burst' if seq<=15 else 'recovery'
                if observed is None:raise RuntimeError('recovered uncreated Batch')
                # All fixture logs in a Batch must use its creation data clock.
                if any(x['observed_time_unix_nano']!=observed for x in logs):raise RuntimeError('collection clock mismatch')
                for label,t in [('ingest',r['received_ns']-observed),('scheduled_ingest',r['received_ns']-scheduled)]:
                    populations[label].append((t/1e6,n));populations[phase+'_'+label].append((t/1e6,n))
                if ack is not None:
                    populations['ack'].append(((ack-observed)/1e6,n));populations['scheduled_ack'].append(((ack-scheduled)/1e6,n))
                sha=hashlib.sha256(raw).digest();clocks.write(json.dumps([i,seq,n,scheduled,observed,r['received_ns'],ack,len(raw),sha.hex()])+'\n')
                transcript.write(json.dumps({'type':'recovered','node_id':b['node_id'].hex(),'generation':b['generation'],'sequence':seq,'bytes':base64.b64encode(sha).decode()})+'\n')
            transcript.write('{"type":"end"}\n')
        with (root/'sim/transcript.jsonl').open() as f:v=delivery_oracle.check(f)
        oracle={'passed':v.passed,'violations':[str(x) for x in v.violations]};dump(root/'oracle.json',oracle)
        stats={k:weighted(v) for k,v in populations.items()};expected=identities*1750
        lag=percentile(creation_lag);srv_peak=max(x['server']['hwm_kib'] for x in samples)/1024;sim_samples=[x for x in samples if x['simulator']];sim_peak=max(x['simulator']['hwm_kib'] for x in sim_samples)/1024
        duration=(samples[-1]['mono_ns']-samples[0]['mono_ns'])/1e9;sim_duration=(sim_samples[-1]['mono_ns']-sim_samples[0]['mono_ns'])/1e9 if len(sim_samples)>1 else None
        offsets=[(x['wall_ns']-x['mono_ns'])/1e6 for x in samples];actual_gen=(max(generated)-epoch)/1e9
        gates={'expected_logs_recovered':total_logs==expected and bad_sizes==0,'expected_batches_generated':len(creates)==identities*25,'pending_zero':sim_summary['undelivered']==0,'delivery_oracle':v.passed,'clean_exits':sim.returncode==server.returncode==0,'creation_lag_p99_le_1s':lag['p99']<=1000,'ingest_p99_le_1s':stats['ingest']['p99']<=1000 and stats['ingest']['negative']==0,'ack_p99_le_1s':stats.get('ack',{}).get('p99',float('inf'))<=1000 and stats.get('ack',{}).get('samples')==expected,'scheduled_ingest_p99_le_2s':stats['scheduled_ingest']['p99']<=2000,'server_rss_le_2gib':srv_peak<=2048,'simulator_rss_le_4gib':sim_peak<=4096,'clock_offset_range_le_5ms':max(offsets)-min(offsets)<=5}
        summary={'name':name,'identities':identities,'seal_workers':workers,'expected_logs':expected,'recovered_logs':total_logs,'created_batches':len(creates),'recovered_batches':recovered,'acks':len(acks),'pending':sim_summary['undelivered'],'creation_lag_ms':lag,'actual_generation_span_seconds':actual_gen,'nominal_average_events_s':expected/25,'events_s_over_actual_generation_span':expected/actual_gen,'encoded_batch_bytes':total_bytes,'encoded_bytes_per_log':total_bytes/total_logs,'latency_ms':stats,'request_rtt_ms':percentile(rtt),'non_ack_attempts':retry,'server_peak_rss_mib':srv_peak,'simulator_peak_rss_mib':sim_peak,'server_mean_cpu_equivalents':(samples[-1]['server']['cpu_s']-samples[0]['server']['cpu_s'])/duration,'simulator_mean_cpu_equivalents':(sim_samples[-1]['simulator']['cpu_s']-sim_samples[0]['simulator']['cpu_s'])/sim_duration if sim_duration else None,'max_sealed_files_waiting':max(x['sealed_files'] for x in samples),'segments_final':samples[-1]['segments'],'sealed_files_final':samples[-1]['sealed_files'],'clock_offset_range_ms':max(offsets)-min(offsets),'peak_trial_disk_bytes':max(x['logical_disk_bytes'] for x in samples),'gates':gates,'passed':all(gates.values()),'oracle':oracle}
        dump(root/'summary.json',summary);print(json.dumps({'name':name,'passed':summary['passed'],'gates':gates,'ingest':stats['ingest'],'scheduled_ingest':stats['scheduled_ingest'],'rss_mib':srv_peak}),flush=True)
        (root/'replay.jsonl').unlink()
        for fname in ['events.jsonl','transcript.jsonl']:
            p=root/'sim'/fname
            with p.open('rb') as src,gzip.open(root/(fname+'.gz'),'wb') as dst:shutil.copyfileobj(src,dst)
        return summary
    finally:
        stop.set()
        for p in children:
            if p.poll() is None:p.kill();p.wait(timeout=10)


def main():
    ap=argparse.ArgumentParser();ap.add_argument('--out',type=Path,required=True);ap.add_argument('--bin-dir',type=Path,required=True);a=ap.parse_args();root=a.out.resolve();bins=a.bin_dir.resolve()
    if root.exists():raise SystemExit('fresh output required')
    cpus=sorted(os.sched_getaffinity(0));assert len(cpus)>=4;root.mkdir(parents=True)
    # Independent digest-oracle control: acknowledged source lacking recovery must fail.
    ident={'node_id':'01'*16,'generation':1,'sequence':1};data=base64.b64encode(hashlib.sha256(b'fixture').digest()).decode()
    control=[{'type':'source',**ident,'bytes':data},{'type':'attempt',**ident,'bytes':data,'injected_conflict':False},{'type':'response',**ident,'kind':'ack','committed_through':1},{'type':'end'}]
    failed=delivery_oracle.check(map(json.dumps,control));assert not failed.passed
    complete=control[:-1]+[{'type':'recovered',**ident,'bytes':data},control[-1]];assert delivery_oracle.check(map(json.dumps,complete)).passed
    dump(root/'negative-control.json',{'missing_recovery_rejected':True,'complete_accepted':True})
    dump(root/'environment.json',{'revision':subprocess.check_output(['git','rev-parse','HEAD'],cwd=REPO,text=True).strip(),'affinity':cpus,'uname':list(os.uname()),'cpu_max':Path('/sys/fs/cgroup/cpu.max').read_text().strip(),'memory_max':Path('/sys/fs/cgroup/memory.max').read_text().strip(),'harness_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),'binaries':{str(p.relative_to(bins)):hashlib.sha256(p.read_bytes()).hexdigest() for p in [bins/'fabric-server',bins/'examples/spindle_sim',bins/'examples/ingest_load',bins/'examples/server_dump']}})
    os.sched_setaffinity(0,cpus[2:4])
    for name,n,w in [('medium',200,1),('enterprise',2000,1),('medium-workers2',200,2)]:
        try:trial(root/name,bins,name,n,w,cpus)
        except Exception as e:dump(root/(name+'-failure.json'),{'error':str(e)});raise
    dump(root/'complete.json',{'sequential':['medium','enterprise','medium-workers2']})

if __name__=='__main__':main()
