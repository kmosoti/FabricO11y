#!/usr/bin/env python3
"""One bounded digitalocean-02 edge cell, with independently graded local custody."""
import argparse
import base64
import copy
import gzip
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import ssl
import subprocess
import sys
import tarfile
import threading
import time
import traceback
import urllib.request

import service
import native
import delivery_oracle

ROOT, HOST = native.REPO, 'digitalocean-02'


def drained(latest,acks,count):
    through = {}
    for label,sequence in acks:
        through[label] = max(through.get(label,0),sequence)
    return len(latest)==count and all(int(v['log_backlog_bytes'])==0 and int(v['batch'])==through.get(label,0)
                                     for label,v in latest.items())


def grade(work, mode, observer):
    records = native.query_oracle.load_records_jsonl(str(work/'recovered.jsonl'))
    verdicts = [native.query_oracle.check(records, q['query'], q['pages']) for q in observer.finals]
    native.dump(work/'query-verdicts.json', verdicts)
    if not all(v['passed'] for v in verdicts):
        raise RuntimeError('independent complete query check failed')
    controls = []
    if not observer.finals or not observer.finals[0]['pages'][0]['rows']:
        raise RuntimeError('actual pipeline query controls require a nonempty first query')
    for defect in ('missing', 'changed'):
        changed = copy.deepcopy(observer.finals[0])
        if defect == 'missing': changed['pages'][0]['rows'].pop(0)
        else: changed['pages'][0]['rows'][0]['body'] += 'changed'
        result = native.query_oracle.check(records, changed['query'], changed['pages'])
        controls.append(dict(defect=defect, query=changed['query'], pages=changed['pages'], verdict=result))
    native.dump(work/'query-negative-controls.json', controls)
    if any(c['verdict']['passed'] for c in controls): raise RuntimeError('actual query pipeline accepted an injected defect')
    batches, seen, duplicates, gaps = {}, {}, 0, []
    with gzip.open(work/'recovered-hashes.jsonl.gz', 'wt') as hashes:
        for r in records:
            raw = base64.b64decode(r['bytes']); b = native.query_oracle.decode_batch(raw)
            key = (r['label'], b['sequence'])
            if key in batches: raise RuntimeError('duplicate recovered Batch')
            sha = hashlib.sha256(raw).hexdigest()
            batches[key] = (sha,b)
            hashes.write(json.dumps([*key, sha, len(raw), r['received_ns']])+'\n')
            gaps.extend(b['gaps'])
            for log in native.query_oracle.decode_logs_request(b['logs_bytes']):
                tag = log['body'].split(' ',1)[0].removeprefix('load-')
                if tag in seen: duplicates += 1
                seen[tag] = hashlib.sha256(log['body'].encode()).hexdigest()
    if mode == 'real':
        with gzip.open(work/'remote/sources.jsonl.gz', 'rt') as source:
            expected = {r[0]:(r[1],r[2],r[3]) for r in map(json.loads,source)}
        acks, retries, latest, rtt = {}, 0, {}, []
        for i in range(8):
            label = f'node{i:02}'
            with gzip.open(work/'remote'/f'{label}-stdout.jsonl.gz', 'rt') as events:
                for event in map(json.loads, events):
                    line = event['line']
                    fields = dict(x.split('=',1) for x in line.split() if '=' in x)
                    if line.startswith('batch='): latest[label] = fields
                    if line.startswith('delivery '):
                        if fields.get('status') == 'ack':
                            key = (label,int(fields['sequence']))
                            if key not in batches or batches[key][0]!=fields['sha256']: raise RuntimeError('individual ACK custody hash mismatch')
                            acks[key] = fields['sha256']
                            rtt.append(int(fields['elapsed_us'])/1000)
                        else: retries += 1
        by_node = {}
        for label,seq in batches: by_node.setdefault(label,[]).append(seq)
        gates = dict(exact_source=not any(native.compare(expected,seen).values()) and len(seen)==len(expected)==33600,
            no_duplicates=duplicates==0, no_gaps=not gaps,
            ack_hashes=len(acks)==len(batches) and all(k in batches and batches[k][0]==h for k,h in acks.items()),
            contiguous=len(by_node)==8 and all(sorted(v)==list(range(1,max(v)+1)) for v in by_node.values()),
            drained=drained(latest,acks,8))
        details = dict(recovered_logs=len(seen), acknowledged_batches=len(acks), retries=retries,
                       request_rtt_ms=native.percentile(rtt), clock_boundary='remote request duration only')
    else:
        transcript = work/'remote/sim/transcript.jsonl'
        with transcript.open() as source: original = list(source)
        recovery = [dict(type='recovered', node_id=b['node_id'].hex(), generation=b['generation'],
                         sequence=key[1], bytes=base64.b64encode(bytes.fromhex(sha)).decode())
                    for key,(sha,b) in batches.items()]
        full = original + [json.dumps(r)+'\n' for r in recovery] + ['{"type":"end"}\n']
        transcript.write_text(''.join(full))
        with gzip.open(work/'delivery-transcript.jsonl.gz','wt') as output: output.writelines(full)
        verdict = delivery_oracle.check(iter(full))
        control = delivery_oracle.check(iter(original+[json.dumps(r)+'\n' for r in recovery[1:]]+['{"type":"end"}\n']))
        native.dump(work/'delivery-verdict.json', dict(passed=verdict.passed, violations=[str(v) for v in verdict.violations],
                    dropped_recovery_rejected=not control.passed, control_violations=[str(v) for v in control.violations]))
        sim = json.loads((work/'remote/sim/sim-summary.json').read_text())
        total_logs = sum(len(native.query_oracle.decode_logs_request(b['logs_bytes'])) for _,b in batches.values())
        gates = dict(delivery_oracle=verdict.passed, dropped_recovery_rejected=bool(recovery) and not control.passed,
                     expected_logs=total_logs==60000, pending_zero=sim['undelivered']==0, no_gaps=not gaps)
        details = dict(recovered_logs=total_logs, simulator=sim)
    native.dump(work/'grading.json', dict(gates=gates, **details))
    if not all(gates.values()): raise RuntimeError('remote exact custody/drain gate failed')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mode', choices=['real','sim'], required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--bin-dir', type=Path, required=True)
    parser.add_argument('--id', required=True)
    parser.add_argument('--workers',type=int,choices=(2,8),default=2)
    args = parser.parse_args()
    service.resource_group.require_limits()
    if not args.id.replace('-','').isalnum(): parser.error('simple fresh id required')
    bins = args.bin_dir.resolve(strict=True)
    hashes, sources = service.frozen_build(bins)
    if args.mode == 'sim': hashes['examples/spindle_sim'] = service.digest(bins/'examples/spindle_sim')
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True)
    scratch.relative_to((service.resource_group.STORAGE/'scratch').resolve(strict=True))
    work, out = scratch/('remote-'+args.id), args.out.resolve()
    work.mkdir(); out.mkdir(parents=True, exist_ok=False)
    os.chmod(work,0o700); os.chmod(out,0o700)
    remote = '/var/tmp/fabric-hammer-'+args.id
    unit = 'fabric-edge-'+args.id
    parent = native.cgroups.delegate()
    page = os.sysconf('SC_PAGE_SIZE')
    group, limits = native.cgroups.subgroup(parent,'hammer-remote-server',4_000_000_000//page*page,3_000_000_000//page*page,256,4)
    cpus = sorted(os.sched_getaffinity(0))
    if len(cpus)<4: raise RuntimeError('four server affinity CPUs required')
    deadline = time.monotonic()+900
    children, samples, errors, commands, auxiliary_units = [], [], [], [], []
    done = threading.Event(); observer = service.observation.Observer()
    status, remote_exists, preserved = 'failed', False, False

    def remaining(cap=30):
        t = min(cap, deadline-time.monotonic()-30)
        if t<=0: raise TimeoutError('remote cell deadline reserve exhausted')
        return t

    def command(argv, check=True):
        try: result = subprocess.run(argv, capture_output=True, timeout=remaining(120))
        except BaseException as e:
            commands.append(dict(argv=argv,exit=None,error=repr(e))); native.dump(out/'commands.json',commands); raise
        number = len(commands)
        (out/f'command-{number:03}.out').write_bytes(result.stdout)
        (out/f'command-{number:03}.err').write_bytes(result.stderr)
        commands.append(dict(argv=argv, exit=result.returncode))
        native.dump(out/'commands.json',commands)
        if check and result.returncode: raise RuntimeError(f'command exit {result.returncode}: {argv[0]}')
        return result

    def ssh(argv, check=True):
        return command(['ssh','-o','BatchMode=yes','-o','ConnectTimeout=10',HOST,shlex.join(list(map(str,argv)))],check)

    def aux(argv):
        name = unit+'-aux'+str(len(auxiliary_units)); auxiliary_units.append(name)
        return ssh(['sudo','-n','systemd-run','--quiet','--wait','--pipe','--collect','--unit='+name,
            '--property=User=dev','--property=MemoryMax=134217728','--property=MemoryHigh=100663296',
            '--property=MemorySwapMax=0','--property=CPUQuota=25%','--property=TasksMax=128',
            '--property=RuntimeMaxSec=90','--property=KillMode=control-group','--property=IOAccounting=yes',*argv])

    def api(endpoint, body=None):
        req = urllib.request.Request(f'https://127.0.0.1:{port}'+endpoint,
            data=json.dumps(body).encode() if body is not None else None,
            headers={'authorization':'Bearer '+admin,'content-type':'application/json'},
            method='POST' if body is not None else 'GET')
        with urllib.request.urlopen(req,context=ctx,timeout=remaining(10)) as response:
            return json.loads(response.read())

    def archive_remote():
        nonlocal preserved
        # Full owned state, including private node credentials on failure, is
        # authenticated before remote deletion; no server key/admin token exists there.
        archive = remote+'.tar.gz'
        aux(['python3','-c',"import pathlib,shutil; r=pathlib.Path("+repr(remote)+"); ps=list(r.rglob('*')); assert not any(p.is_symlink() for p in ps); n=sum(p.stat().st_size for p in ps if p.is_file()); bound=n+4096*len(ps)+1048576; assert n+bound<=512*2**20, 'raw plus conservative archive exceeds512MiB'; assert shutil.disk_usage(r).free>=2*2**30+bound, 'archive would violate2GiBreserve'; print(n,bound)"])
        aux(['tar','-czf',archive,'-C',remote,'.'])
        check = aux(['python3','-c',"import hashlib,pathlib; p=pathlib.Path("+repr(archive)+"); print(p.stat().st_size,hashlib.file_digest(p.open('rb'),'sha256').hexdigest())"])
        size, digest = check.stdout.decode().split()
        if int(size)>512*2**20: raise RuntimeError('remote archive exceeds512MiB')
        command(['scp','-q',f'{HOST}:{archive}',str(out/'remote.tar.gz')])
        if service.digest(out/'remote.tar.gz')!=digest or (out/'remote.tar.gz').stat().st_size!=int(size):
            raise RuntimeError('remote archive transfer hash mismatch')
        with tarfile.open(out/'remote.tar.gz') as tar:
            for member in tar.getmembers():
                if not (member.isfile() or member.isdir()) or Path(member.name).is_absolute() or '..' in Path(member.name).parts:
                    raise RuntimeError('unsafe remote archive member')
            tar.extractall(work/'remote',filter='data')
        native.dump(out/'remote-archive.json',dict(bytes=int(size),sha256=digest,remote_readback=True))
        preserved = True
        ssh(['rm','-rf','--',remote,archive])
        if ssh(['test','!','-e',remote],False).returncode: raise RuntimeError('remote owned cleanup failed')

    sampler = None
    try:
        native.dump(out/'environment.json',dict(command=sys.argv,host=HOST,mode=args.mode,remote=remote,sim_workers=args.workers,
            binary_sha256=hashes,source_sha256=sources,server_limits=limits,
            protocol_sha256=service.digest(ROOT/'docs/experiments/benchmarks/hammer-reference-protocol.md'),
            reused_helpers_sha256={p.name:service.digest(p) for p in map(Path,[service.__file__,native.__file__,service.observation.__file__,native.query_oracle.__file__,delivery_oracle.__file__,native.cgroups.__file__])},
            harness_sha256={p.name:service.digest(p) for p in [Path(__file__),Path(__file__).with_name('remote_worker.py')]},
            clock_boundary='remote/source and local/query clocks not assumed synchronized'))
        native.dump(out/'checker-controls.json',service.observation.controls())
        for p in [Path(__file__),Path(__file__).with_name('remote_worker.py')]: shutil.copyfile(p,out/p.name)
        pre = ssh(['python3','-c',"import os,pathlib,shutil,socket,json; print(json.dumps({'host':socket.gethostname(),'uid':os.getuid(),'cpus':os.cpu_count(),'mem':pathlib.Path('/proc/meminfo').read_text(),'disk':shutil.disk_usage('/var/tmp')._asdict()}))"])
        host = json.loads(pre.stdout)
        if host['host']!='digitalocean-02' or host['disk']['free']<2*2**30 or ssh(['id','-un']).stdout.strip()!=b'dev':
            raise RuntimeError('remote host/identity/disk admission failed')
        native.make_certs(work); port = native.free_port(); admin = os.urandom(32).hex()
        (work/'admin-token').write_text(admin+'\n'); os.chmod(work/'admin-token',0o600)
        conf = work/'server.conf'
        conf.write_text(f'listen=127.0.0.1:{port}\ntls_cert={work}/server.pem\ntls_key={work}/server.key\nstate_dir={work}/state\nadmin_token_file={work}/admin-token\njournal_bytes=4294967296\njournal_file_bytes=67108864\nretention_s=86400\nretention_bytes=4294967296\nseal_workers=1\n')
        ctx = ssl.create_default_context(cafile=str(work/'ca.pem'))
        server_argv = [sys.executable,str(Path(native.__file__)), '--enter',str(group),'taskset','-c',','.join(map(str,cpus[:4])),str(bins/'fabric-server'),'serve',str(conf)]
        server = subprocess.Popen(server_argv,stdout=open(work/'server.out','wb'),stderr=open(work/'server.err','wb')); children.append(server)
        commands.append(dict(argv=server_argv,exit=None))
        for _ in range(200):
            if server.poll() is not None: raise RuntimeError('local server startup failed')
            try: api('/v1/admin/nodes'); break
            except OSError: time.sleep(.05)
        else: raise RuntimeError('local TLS readiness deadline')
        ssh(['mkdir','-m','700',remote]); remote_exists = True
        stage = work/'transfer'; stage.mkdir(mode=0o700)
        shutil.copyfile(work/'ca.pem',stage/'ca.pem')
        shutil.copyfile(Path(__file__).with_name('remote_worker.py'),stage/'remote_worker.py')
        binary = 'fabric-node' if args.mode=='real' else 'spindle_sim'
        shutil.copyfile(bins/('fabric-node' if args.mode=='real' else 'examples/spindle_sim'),stage/binary); os.chmod(stage/binary,0o755)
        rp = int(ssh(['python3','-c',"import socket; s=socket.socket();\nfor p in range(39100,39200):\n try: s.bind(('127.0.0.1',p)); print(p); break\n except OSError: pass\nelse: raise RuntimeError('no free loopback reverse port')"]).stdout)
        tokens = []
        for i in range(8 if args.mode=='real' else 100):
            label = f'node{i:02}' if args.mode=='real' else f'sim{i:04}'
            logs = [remote+'/'+label+'.log'] if args.mode=='real' else []
            token = api('/v1/admin/nodes',dict(name=label,metric_interval_s=15,logs=logs))['token']; tokens.append(token)
            if args.mode=='real':
                (stage/(label+'.log')).touch()
                (stage/(label+'.token')).write_text(token+'\n')
                (stage/(label+'.conf')).write_text(f'spool_dir={remote}/{label}-spool\nlog={remote}/{label}.log\nmetric_interval_s=15\nspool_bytes=16777216\nserver_url=https://127.0.0.1:{rp}\nserver_ca={remote}/ca.pem\ntoken_file={remote}/{label}.token\n')
        if args.mode=='sim': (stage/'tokens').write_text('\n'.join(tokens)+'\n')
        for p in stage.iterdir():
            if p.suffix=='.token' or p.name=='tokens': os.chmod(p,0o600)
        manifest = {p.name:service.digest(p) for p in stage.iterdir()}
        native.dump(out/'transfer-sha256.json',manifest)
        command(['scp','-q',*[str(p) for p in stage.iterdir()],f'{HOST}:{remote}/'])
        actual = json.loads(aux(['python3','-c',"import pathlib,hashlib,json; r=pathlib.Path("+repr(remote)+");print(json.dumps({p.name:hashlib.file_digest(p.open('rb'),'sha256').hexdigest() for p in r.iterdir() if p.is_file()}))"]).stdout)
        if actual!=manifest: raise RuntimeError('remote input transfer/readback hash mismatch')
        tunnel_argv = ['ssh','-o','BatchMode=yes','-o','ExitOnForwardFailure=yes','-o','ServerAliveInterval=10','-o','ServerAliveCountMax=2','-N','-R',f'127.0.0.1:{rp}:127.0.0.1:{port}',HOST]
        tunnel = subprocess.Popen(tunnel_argv,stdout=open(work/'tunnel.out','wb'),stderr=open(work/'tunnel.err','wb')); children.append(tunnel); commands.append(dict(argv=tunnel_argv,exit=None))
        time.sleep(.5)
        if tunnel.poll() is not None: raise RuntimeError('loopback reverse tunnel failed')
        observer.start(api,time.time_ns(),8 if args.mode=='real' else 100)
        def sample():
            while not done.is_set():
                try:
                    if native.footprint(work)>8*2**30 or shutil.disk_usage(work).free<16*2**30: raise RuntimeError('local remote-cell disk admission failed')
                    samples.append(dict(wall_ns=time.time_ns(),mono_ns=time.monotonic_ns(),server=native.process_stats(server.pid),
                        cgroup={k:(group/k).read_text().strip() for k in ['memory.current','memory.peak','memory.events','memory.stat','cpu.stat','io.stat','memory.swap.current']},
                        state_bytes=native.footprint(work/'state')))
                except Exception as e: errors.append(repr(e)); done.set()
                done.wait(1)
        sampler = threading.Thread(target=sample); sampler.start()
        invocation = ['sudo','-n','systemd-run','--wait','--pipe','--collect','--unit='+unit,
            '--property=User=dev','--property=WorkingDirectory='+remote,'--property=MemoryMax=134217728',
            '--property=MemoryHigh=100663296','--property=MemorySwapMax=0','--property=CPUQuota=50%',
            '--property=TasksMax=128','--property=RuntimeMaxSec=600','--property=KillMode=control-group','--property=IOAccounting=yes',
            'python3',remote+'/remote_worker.py','--mode',args.mode,'--work',remote,'--port',str(rp),'--workers',str(args.workers)]
        # Finite remote600s unit runs while local consumer and sampler remain active.
        argv = ['ssh','-o','BatchMode=yes','-o','ConnectTimeout=10',HOST,shlex.join(invocation)]
        try: result = subprocess.run(argv,stdout=open(out/'remote-service.out','wb'),stderr=open(out/'remote-service.err','wb'),timeout=remaining(650))
        except BaseException as e:
            commands.append(dict(argv=argv,exit=None,error=repr(e))); raise
        commands.append(dict(argv=argv,exit=result.returncode)); native.dump(out/'commands.json',commands)
        observer.stop()
        native.dump(work/'live-query-summary.json', dict(requests=len(observer.queries),
            request_errors=sum(q['error'] is not None for q in observer.queries),
            incomplete_answers=sum(q['answer'] is not None and not q['answer'].get('complete',False) for q in observer.queries),
            unavailable_answers=sum(q['answer'] is not None and bool(q['answer'].get('unavailable')) for q in observer.queries),
            boundary='local HTTP request start/end and elapsed monotonic duration; remote clock relation unknown'))
        archive_remote()
        if result.returncode or errors: raise RuntimeError('remote workload/observer failed: '+repr((result.returncode,errors)))
        label = 'node00' if args.mode=='real' else 'sim0000'
        queries = [dict(service.observation.QUERY,node=label,contains='load-00:000' if args.mode=='real' else ''),
                   dict(service.observation.QUERY,contains='ABSENT-profile-sentinel'),
                   dict(service.observation.QUERY,node=label,kind='metrics',name=service.observation.METRIC if args.mode=='real' else 'sim.metric.0')]
        if args.mode=='real': queries.append(dict(service.observation.QUERY,node=label,contains='load-00:0000:00 ',limit=2))
        for query in queries:
            pages, current = [], query
            for _ in range(200):
                page = api('/v1/admin/query',current); pages.append(page)
                if page['next_page'] is None: break
                current = dict(query,page=page['next_page'])
            else: raise RuntimeError('remote final query pagination exceeded200pages')
            observer.finals.append(dict(query=query,pages=pages))
        observer.archive(work)
        done.set(); sampler.join(5)
        server.send_signal(signal.SIGTERM); server.wait(timeout=remaining(30))
        if server.returncode: raise RuntimeError('local server graceful exit failed')
        with (work/'recovered.jsonl').open('wb') as output:
            command_result = subprocess.run([str(bins/'examples/server_dump'),str(conf),'--records'],stdout=output,stderr=open(work/'dump.err','wb'),timeout=remaining(120))
        commands.append(dict(argv=[str(bins/'examples/server_dump'),str(conf),'--records'],exit=command_result.returncode))
        if command_result.returncode: raise RuntimeError('local durable replay failed')
        grade(work,args.mode,observer)
        checked, checked_sources = service.frozen_build(bins)
        if checked_sources!=sources or any(checked[k]!=hashes[k] for k in checked): raise RuntimeError('source/binary identity changed during remote cell')
        worker = json.loads((work/'remote/worker-result.json').read_text())
        events = {k:int(v) for k,v in (s.split() for s in (group/'memory.events').read_text().splitlines())}
        remote_events = {k:int(v) for k,v in (s.split() for s in worker['final_cgroup']['memory.events'].splitlines())}
        if any(e['oom'] or e['oom_kill'] for e in [events,remote_events]): raise RuntimeError('child cgroup OOM recorded')
        status = 'passed'
    except BaseException as e:
        native.dump(out/'failure.json',dict(error=repr(e),traceback=traceback.format_exc(),original_remote=remote))
    finally:
        done.set()
        try: observer.stop(); observer.archive(work)
        except Exception as e: native.dump(out/'observer-cleanup-error.json',dict(error=repr(e)))
        if sampler: sampler.join(5)
        for p in children:
            if p.poll() is None: p.terminate()
            try: p.wait(timeout=10)
            except subprocess.TimeoutExpired: p.kill(); p.wait(timeout=5)
        for c in commands:
            for p in children:
                if c['argv']==p.args: c['exit']=p.returncode
        try:
            ssh(['sudo','-n','systemctl','stop',unit],False)
            active = ssh(['systemctl','is-active',unit],False).stdout.strip()
            if active in (b'active',b'activating',b'deactivating'): raise RuntimeError('owned remote unit still active')
            for auxiliary in auxiliary_units:
                ssh(['sudo','-n','systemctl','stop',auxiliary],False)
                if ssh(['systemctl','is-active',auxiliary],False).stdout.strip() in (b'active',b'activating',b'deactivating'):
                    raise RuntimeError('owned auxiliary unit still active')
            if remote_exists and not preserved: archive_remote()
        except Exception as e:
            status='failed'; native.dump(out/'remote-cleanup-error.json',dict(error=repr(e),remote_preserved=remote))
        native.dump(work/'local-resources.json',samples)
        native.dump(out/'commands.json',commands)
        native.dump(out/'cgroup-final.json',native.cgroups.snapshot(parent))
        if not (group/'cgroup.procs').read_text().strip(): group.rmdir()
        if status=='passed':
            for p in work.iterdir():
                if p.is_file() and p.suffix in ('.json','.gz','.out','.err'): shutil.copyfile(p,out/p.name)
            shutil.rmtree(work)
        else: native.dump(out/'failure-tree-members.json',service.inventory(work))
        native.dump(out/'cleanup.json',dict(status=status,local_removed=not work.exists(),remote_preserved=not preserved,
            local_failure_scratch=str(work) if work.exists() else None,owned_children_exits=[p.returncode for p in children]))
        native.dump(out/'sha256.json',{p.name:service.digest(p) for p in out.iterdir() if p.is_file() and p.name!='sha256.json'})
    return 0 if status=='passed' else 1


if __name__ == '__main__':
    raise SystemExit(main())
