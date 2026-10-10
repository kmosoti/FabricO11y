"""Finite effective private-path throughput, without disk fixtures."""
import hashlib
import json
import os
from pathlib import Path
import shlex
import statistics
import subprocess
import sys
import threading
import time

import service

REMOTE = r'''
import hashlib,json,os,pathlib,resource,sys,time
g=pathlib.Path('/sys/fs/cgroup')/next(s[3:] for s in pathlib.Path('/proc/self/cgroup').read_text().splitlines() if s.startswith('0::')).lstrip('/')
limits={k:(g/k).read_text().strip() for k in ['memory.max','memory.high','memory.swap.max','pids.max','cpu.max']}
assert limits['memory.max']=='134217728' and limits['memory.swap.max']=='0' and limits['pids.max']=='128'
a,b=map(int,limits['cpu.max'].split());assert a/b==.5
inp,out=sys.stdin.buffer,sys.stdout.buffer
def emit(obj):out.write(json.dumps(obj).encode()+b'\n');out.flush()
emit({'ready':True,'limits':limits,'host':os.uname().nodename})
for _ in range(30):
 data=inp.read(1);assert data==b'X';out.write(data);out.flush()
block=bytes(range(256))*4096
n=16*len(block)
for trial in range(3):
 assert inp.read(1)==b'U'
 began=time.perf_counter();digest=hashlib.sha256();remaining=n
 while remaining:
  data=inp.read(min(65536,remaining));assert data;remaining-=len(data);digest.update(data)
 emit({'direction':'upload','bytes':n,'sha256':digest.hexdigest(),'endpoint_seconds':time.perf_counter()-began})
 assert inp.read(1)==b'D'
 for _ in range(16):out.write(block)
 out.flush()
emit({'cpu_seconds':resource.getrusage(resource.RUSAGE_SELF).ru_utime+resource.getrusage(resource.RUSAGE_SELF).ru_stime,'peak_rss_kib':resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,'memory_peak':(g/'memory.peak').read_text().strip(),'memory_events':(g/'memory.events').read_text().strip()})
'''


def main():
    service.resource_group.require_limits()
    out=Path(sys.argv[1]);out.mkdir(parents=True,exist_ok=False)
    host='digitalocean-02'
    unit='fabric-edge-network-'+str(os.getpid())
    route=['tailscale','ping','--c','5','--until-direct=false',host]
    route_result=subprocess.run(route,capture_output=True,text=True,timeout=45)
    (out/'tailscale-ping.txt').write_text(route_result.stdout+route_result.stderr)
    cmd=['ssh','-o','BatchMode=yes','-o','Compression=no','-o','ConnectTimeout=10',host,
         shlex.join(['sudo','-n','systemd-run','--quiet','--wait','--pipe','--collect','--unit='+unit,
          '--property=User=dev','--property=MemoryMax=134217728','--property=MemoryHigh=100663296',
          '--property=MemorySwapMax=0','--property=CPUQuota=50%','--property=TasksMax=128',
          '--property=RuntimeMaxSec=180','--property=KillMode=control-group','python3','-u','-c',REMOTE])]
    p=None; timer=None; start=time.perf_counter();result=dict(argv=cmd,route_argv=route,route_exit=route_result.returncode)
    (out/'network.py').write_bytes(Path(__file__).read_bytes())
    try:
        p=subprocess.Popen(cmd,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=open(out/'ssh.err','wb'))
        timer=threading.Timer(175,p.kill);timer.start()
        ready=json.loads(p.stdout.readline());assert ready['ready'] and ready['host']==host
        result.update(handshake_seconds=time.perf_counter()-start,remote=ready)
        latencies=[]
        for _ in range(30):
            before=time.perf_counter();p.stdin.write(b'X');p.stdin.flush();assert p.stdout.read(1)==b'X'
            latencies.append((time.perf_counter()-before)*1000)
        block=bytes(range(256))*4096
        expected=hashlib.sha256(block*16).hexdigest()
        trials=[]
        for trial in range(3):
            before=time.perf_counter();p.stdin.write(b'U')
            for _ in range(16):p.stdin.write(block)
            p.stdin.flush();ack=json.loads(p.stdout.readline());elapsed=time.perf_counter()-before
            assert ack['bytes']==16*2**20 and ack['sha256']==expected
            trials.append(dict(trial=trial,direction='upload',seconds=elapsed,mbps=ack['bytes']*8/elapsed/1e6,remote_ack=ack))
            before=time.perf_counter();p.stdin.write(b'D');p.stdin.flush();digest=hashlib.sha256();remaining=16*2**20
            while remaining:
                data=p.stdout.read(min(65536,remaining));assert data;remaining-=len(data);digest.update(data)
            elapsed=time.perf_counter()-before;assert digest.hexdigest()==expected
            trials.append(dict(trial=trial,direction='download',seconds=elapsed,mbps=16*2**20*8/elapsed/1e6,sha256=digest.hexdigest()))
        result.update(round_trip_ms=latencies,round_trip_median_ms=statistics.median(latencies),
                      round_trip_max_ms=max(latencies),trials=trials,remote_final=json.loads(p.stdout.readline()))
        p.stdin.close();result['exit']=p.wait(timeout=10);assert result['exit']==0
        result['effective_mbps']={d:statistics.median(t['mbps'] for t in trials if t['direction']==d) for d in ('upload','download')}
        result['boundary']='16MiB payload transfers through SSH over Tailscale, compression disabled, includes endpoint CPU and SSH encryption; not raw link capacity'
        print(json.dumps({k:result[k] for k in ('round_trip_median_ms','round_trip_max_ms','effective_mbps','boundary')}))
    finally:
        if timer:timer.cancel()
        if p and p.poll() is None:p.kill();p.wait(timeout=10)
        cleanup=subprocess.run(['ssh','-o','BatchMode=yes','-o','ConnectTimeout=10',host,
            shlex.join(['sudo','-n','systemctl','stop',unit])],capture_output=True,text=True,timeout=20)
        active=subprocess.run(['ssh','-o','BatchMode=yes','-o','ConnectTimeout=10',host,
            shlex.join(['systemctl','is-active',unit])],capture_output=True,text=True,timeout=20)
        result.update(cleanup_exit=cleanup.returncode,final_unit_state=active.stdout.strip(),elapsed_seconds=time.perf_counter()-start)
        (out/'summary.json').write_text(json.dumps(result,indent=2)+'\n')
        if active.stdout.strip() in ('active','activating','deactivating'):raise RuntimeError('remote network service remains active')


if __name__=='__main__':main()
