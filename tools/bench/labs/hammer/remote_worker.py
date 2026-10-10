#!/usr/bin/env python3
"""Owned remote edge worker: no workloads before actual containment admission."""
import argparse
import base64
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import threading
import time
import traceback


def dump(path, value):
    path.write_text(json.dumps(value, indent=2)+'\n')


def footprint(root):
    total = 0
    for p in root.rglob('*'):
        if p.is_symlink():
            raise RuntimeError('scratch symlink rejected')
        try:
            if p.is_file():
                total += p.stat().st_size
        except FileNotFoundError:
            pass
    return total


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mode', choices=['real', 'sim'], required=True)
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--port', type=int, required=True)
    parser.add_argument('--workers',type=int,choices=(2,8),default=2)
    args = parser.parse_args()
    root = args.work.resolve(strict=True)
    if root.parent != Path('/var/tmp') or not root.name.startswith('fabric-hammer-') or root.stat().st_uid != os.getuid():
        raise RuntimeError('fresh owned disk-backed remote path required')
    group = Path('/sys/fs/cgroup')/next(s[3:] for s in Path('/proc/self/cgroup').read_text().splitlines() if s.startswith('0::')).lstrip('/')
    wanted = {'memory.max': '134217728', 'memory.high': '100663296', 'memory.swap.max': '0', 'pids.max': '128'}
    actual = {k: (group/k).read_text().strip() for k in wanted}
    cpu = list(map(int, (group/'cpu.max').read_text().split()))
    if actual != wanted or cpu[0]/cpu[1] != .5 or not group.name.startswith('fabric-edge-'):
        raise RuntimeError('remote enforced cgroup limits differ')
    runtime = subprocess.check_output(['systemctl', 'show', group.name, '-p', 'RuntimeMaxUSec', '--value'], text=True).strip()
    if runtime != '10min':
        raise RuntimeError('remote600s service deadline not enforced: '+runtime)
    for name in ('memory.current','memory.peak','memory.events','memory.stat','memory.swap.current','cpu.stat','io.stat','pids.current'):
        (group/name).read_text()
    dump(root/'limits.json', dict(cgroup=str(group), actual=actual, cpu=cpu, runtime=runtime,
        uname=list(os.uname()), memory=Path('/proc/meminfo').read_text(), free_bytes=shutil.disk_usage(root).free))
    start, deadline = time.monotonic(), time.monotonic()+540
    kids, readers, resources, errors, commands = [], [], [], [], []
    stop = threading.Event()

    def admission():
        if footprint(root)>=512*2**20 or shutil.disk_usage(root).free<2*2**30 or time.monotonic()>deadline:
            raise RuntimeError('remote scratch/free disk/deadline boundary')
        if any(p.poll() is not None for p in kids):
            raise RuntimeError('remote child exited early')

    def spawn(binary, argv, label):
        command = [str(root/binary), *map(str, argv)]
        p = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=open(root/(label+'.err'), 'wb'))
        kids.append(p); commands.append(dict(argv=command, label=label, exit=None))
        def read():
            with gzip.open(root/(label+'-stdout.jsonl.gz'), 'wt') as output:
                for raw in iter(p.stdout.readline, b''):
                    output.write(json.dumps(dict(wall_ns=time.time_ns(), mono_ns=time.monotonic_ns(), line=raw.decode(errors='replace').rstrip()))+'\n')
        t = threading.Thread(target=read); t.start(); readers.append(t)
        return p

    def sample():
        while not stop.is_set():
            try:
                admission()
                processes = []
                for p in kids:
                    status = Path(f'/proc/{p.pid}/status').read_text()
                    stat = Path(f'/proc/{p.pid}/stat').read_text().rpartition(') ')[2].split()
                    processes.append(dict(pid=p.pid, cpu_s=(int(stat[11])+int(stat[12]))/os.sysconf('SC_CLK_TCK'),
                        rss_kib=next(int(s.split()[1]) for s in status.splitlines() if s.startswith('VmRSS:'))))
                resources.append(dict(wall_ns=time.time_ns(), mono_ns=time.monotonic_ns(), processes=processes,
                    disk_bytes=footprint(root), cgroup={k:(group/k).read_text().strip() for k in
                    ['memory.current','memory.peak','memory.events','memory.stat','memory.swap.current','cpu.stat','io.stat','pids.current']}))
            except Exception as e:
                errors.append(str(e)); dump(root/'sampler-failure.json',dict(error=str(e),traceback=traceback.format_exc())); stop.set(); return
            stop.wait(1)
    sampler = None
    status = 'failed'
    try:
        admission()
        if args.mode == 'real':
            for i in range(8):
                spawn('fabric-node', ['run', root/f'node{i:02}.conf'], f'node{i:02}')
        else:
            spawn('spindle_sim', ['--server-url', f'https://127.0.0.1:{args.port}', '--ca', root/'ca.pem',
                '--tokens', root/'tokens', '--seed', '0xA11FA001', '--seconds', '60', '--workers', str(args.workers),
                '--log-factor', '5', '--body-bytes', '900', '--out', root/'sim'], 'sim')
        sampler = threading.Thread(target=sample); sampler.start()
        if args.mode == 'real':
            offered = 0
            epoch = time.monotonic()
            with gzip.open(root/'sources.jsonl.gz', 'wt') as source:
                for tick in range(600):
                    stop.wait(max(0, epoch+tick*.1-time.monotonic()))
                    if errors: raise RuntimeError(';'.join(errors))
                    admission()
                    late = max(0, time.monotonic()-(epoch+tick*.1))
                    for i in range(8):
                        with (root/f'node{i:02}.log').open('ab', buffering=0) as logs:
                            for j in range((1,4,16)[tick//200]):
                                tag = f'{i:02}:{tick:04}:{j:02}'
                                padding = ('R'*900 if (tick+j)%2 == 0 else base64.b85encode(hashlib.shake_256(f'2704201:{tag}'.encode()).digest(720)).decode())
                                body = ('load-'+tag+' '+padding)[:900]
                                logs.write(body.encode()+b'\n')
                                source.write(json.dumps([tag, hashlib.sha256(body.encode()).hexdigest(), time.time_ns(), tick//200, late])+'\n')
                                offered += 1
            stop.wait(60)
            if errors: raise RuntimeError(';'.join(errors))
            stop.set(); sampler.join(5)
            for p in kids: p.send_signal(signal.SIGTERM)
        else:
            # Simulator natural completion is expected, so only admission's
            # early-exit check is disabled after detecting its completion.
            while kids[0].poll() is None and not stop.wait(.2):
                if time.monotonic()>deadline: raise TimeoutError('remote simulator deadline')
            stop.set(); sampler.join(5)
            # A zero simulator completion can race the sampler's early-exit
            # diagnostic; distinguish that expected event from all other errors.
            errors = [e for e in errors if 'remote child exited early' not in e]
            offered = 60000
        for p in kids: p.wait(timeout=max(.1, min(30, deadline-time.monotonic())))
        if errors or any(p.returncode != 0 for p in kids):
            raise RuntimeError('remote exits/errors: '+repr((errors,[p.returncode for p in kids])))
        status = 'passed'
        dump(root/'worker-result.json', dict(status=status, offered_logs=offered, elapsed_s=time.monotonic()-start,
            final_cgroup={k:(group/k).read_text().strip() for k in ['memory.events','memory.peak','memory.swap.current']},
            clock_boundary='remote clocks only; no synchronized cross-host latency claim'))
    except BaseException as e:
        dump(root/'worker-failure.json', dict(error=repr(e), elapsed_s=time.monotonic()-start))
    finally:
        stop.set()
        for p in kids:
            if p.poll() is None: p.kill(); p.wait(timeout=10)
        for t in readers: t.join(5)
        if sampler: sampler.join(5)
        for c,p in zip(commands,kids): c['exit'] = p.returncode
        dump(root/'commands.json', commands)
        dump(root/'resources.json', resources)
    return 0 if status == 'passed' else 1


if __name__ == '__main__':
    raise SystemExit(main())
