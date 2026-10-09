#!/usr/bin/env python3
"""Finite 20-collector pressure workload; exact replay and data clocks."""
import argparse
import base64
import gzip
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import signal
import socket
import ssl
import subprocess
import sys
import threading
import time
import urllib.request

REPO = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(REPO / 'tools/qualification'))
from delivery_faults import free_port, make_certs
import query_oracle
sys.path.insert(0, str(REPO / "tools/bench/labs/completion"))
import cgroups


def dump(path, obj):
    path.write_text(json.dumps(obj, indent=2) + '\n')


def percentile(values):
    a = sorted(values)
    return {'samples': len(a), 'p50': a[math.ceil(.5*len(a))-1] if a else None,
            'p99': a[math.ceil(.99*len(a))-1] if a else None,
            'max': a[-1] if a else None, 'negative': sum(x < 0 for x in a)}


def compare(expected, seen):
    return {'missing': len(expected.keys() - seen.keys()),
            'unexpected': len(seen.keys() - expected.keys()),
            'changed': sum(expected[k][0] != seen[k] for k in expected.keys() & seen.keys())}


def footprint(path):
    total = 0
    for p in path.rglob('*'):
        try:
            if p.is_file():
                total += p.stat().st_size
        except FileNotFoundError:
            # Concurrent journal/Spool rename and reclaim is normal.
            continue
    return total


def process_stats(pid):
    fields = Path(f'/proc/{pid}/stat').read_text().rpartition(') ')[2].split()
    status = dict((s.split(':', 1)[0], s.split(':', 1)[1].strip()) for s in Path(f'/proc/{pid}/status').read_text().splitlines() if ':' in s)
    return {'cpu_s': (int(fields[11]) + int(fields[12])) / os.sysconf('SC_CLK_TCK'),
            'rss_kib': int(status['VmRSS'].split()[0]), 'hwm_kib': int(status['VmHWM'].split()[0])}


def trial(root, bins, name, server_cpus, node_cpus, observer=None, query_plan=None, groups=None, deadline=None, seed=2703163393):
    def remaining(cap, reserve=20):
        value = min(cap, deadline - time.monotonic() - reserve)
        if value <= 0:
            raise TimeoutError("absolute cell deadline cleanup reserve reached")
        return value
    root.mkdir()
    (root / 'owned').write_text('hammer pressure owned scratch\n')
    nodes_count = 20
    phase_per_tick = (5, 20, 80)
    make_certs(root)
    port = free_port()
    admin = os.urandom(32).hex()
    (root / 'admin-token').write_text(admin+'\n')
    conf = root / 'server.conf'
    conf.write_text(f'listen=127.0.0.1:{port}\ntls_cert={root}/server.pem\ntls_key={root}/server.key\nstate_dir={root}/state\nadmin_token_file={root}/admin-token\njournal_bytes=4294967296\njournal_file_bytes=67108864\nretention_s=86400\nretention_bytes=4294967296\nseal_workers=1\n')
    if query_plan is not None:
        if query_plan not in ('scan', 'walk'):
            raise ValueError('query_plan must be scan or walk')
        with conf.open('a') as stream:
            stream.write(f'query_plan={query_plan}\n')
    kids, readers, node_events, samples, expected = [], [], {}, [], {}
    stop = threading.Event()
    errors = []
    started = time.monotonic()
    ctx = ssl.create_default_context(cafile=str(root/'ca.pem'))
    logs = []

    def spawn(binary, arguments, cpus, label, piped=False):
        group = groups['server' if label == 'server' else 'nodes']
        p = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), '--enter', str(group), 'taskset', '-c', ','.join(map(str, cpus)), str(binary), *map(str, arguments)],
                             stdout=subprocess.PIPE if piped else open(root/(label+'.out'), 'wb'), stderr=open(root/(label+'.err'), 'wb'), bufsize=0)
        kids.append(p)
        return p

    def api(endpoint, body):
        req = urllib.request.Request(f'https://127.0.0.1:{port}'+endpoint, data=json.dumps(body).encode(),
            headers={'authorization': 'Bearer '+admin, 'content-type': 'application/json'}, method='POST')
        with urllib.request.urlopen(req, context=ctx, timeout=remaining(10, 40)) as r:
            return json.loads(r.read())

    def read_node(p, label):
        events = node_events[label] = []
        with gzip.open(root/(label+'-events.jsonl.gz'), 'wt') as out:
            for raw in iter(p.stdout.readline, b''):
                t = time.time_ns()
                line = raw.decode().strip()
                if line.startswith('delivery ') or line.startswith('batch='):
                    fields = dict(x.split('=', 1) for x in line.split() if '=' in x)
                    e = {'t': t, 'kind': 'ack_attempt' if line.startswith('delivery ') else 'cycle', **fields}
                    events.append(e)
                    out.write(json.dumps(e)+'\n')

    try:
        server = spawn(bins/'fabric-server', ['serve', conf], server_cpus, 'server')
        for _ in range(200):
            if server.poll() is not None:
                raise RuntimeError('server exited during startup')
            try:
                req = urllib.request.Request(f'https://127.0.0.1:{port}/v1/admin/nodes', headers={'authorization': 'Bearer '+admin})
                with urllib.request.urlopen(req, context=ctx, timeout=remaining(1, 40)) as response:
                    json.loads(response.read())
                break
            except OSError:
                time.sleep(.05)
        else:
            raise RuntimeError('server startup timeout')
        for i in range(nodes_count):
            label = f'node{i:02}'
            path = root/(label+'.log'); path.touch(); logs.append(open(path, 'ab', buffering=0))
            token = api('/v1/admin/nodes', {'name': label, 'metric_interval_s': 15, 'logs': [str(path)]})['token']
            (root/(label+'.token')).write_text(token+'\n')
            nconf = root/(label+'.conf')
            nconf.write_text(f'spool_dir={root/(label+"-spool")}\nlog={path}\nmetric_interval_s=15\nspool_bytes=268435456\nserver_url=https://127.0.0.1:{port}\nserver_ca={root}/ca.pem\ntoken_file={root/(label+".token")}\n')
            p = spawn(bins/'fabric-node', ['run', nconf], node_cpus, label, True)
            thread = threading.Thread(target=read_node, args=(p, label)); thread.start(); readers.append(thread)

        def sample():
            while not stop.is_set():
                try:
                    if time.monotonic() > deadline - 800:
                        raise RuntimeError('absolute cell deadline verification reserve reached')
                    if any(p.poll() is not None for p in kids):
                        raise RuntimeError('child exited before shutdown')
                    size = footprint(root)
                    if footprint(Path(os.environ['FABRIC_SCRATCH_ROOT'])) > 8*2**30 or shutil.disk_usage(root).free < 16*2**30:
                        raise RuntimeError('disk bound reached')
                    journal = root/'state/journal'
                    wall = time.time_ns(); mono = time.monotonic_ns()
                    row = {'wall_ns': wall, 'mono_ns': mono, 'state_bytes': size,
                        'server': process_stats(server.pid), 'nodes': [process_stats(p.pid) for p in kids[1:]],
                        'journal_bytes': footprint(journal),
                        'sealed_files': len(list(journal.glob('sealed-*.faj'))),
                        'segments': len(list((root/'state/segments').glob('seg-*')))}
                    if observer:
                        row.update(observer.sample(root, kids, node_events))
                        row['child_cgroups'] = {k: {n: (g/n).read_text().strip() for n in ['memory.max','memory.high','memory.current','memory.peak','memory.events','memory.stat','memory.swap.max','memory.swap.current','cpu.max','cpu.stat','io.stat','pids.max','pids.current']} for k,g in groups.items()}
                    samples.append(row)
                except Exception as e:
                    errors.append(str(e)); stop.set(); return
                stop.wait(1)
        sampler = threading.Thread(target=sample); sampler.start()
        # Startup wait remains bounded and interruptible.
        stop.wait(5)
        offer_start = time.monotonic()
        if observer:
            observer.start(api, time.time_ns(), nodes_count)
        with gzip.open(root/'sources.jsonl.gz', 'wt') as src:
            for tick in range(1800):
                stop.wait(max(0, offer_start+tick*.1-time.monotonic()))
                if stop.is_set():
                    raise RuntimeError('; '.join(errors))
                phase = 'normal' if tick < 600 else 'burst' if tick < 1200 else 'recovery'
                per_tick = phase_per_tick[tick // 600]
                late_ms = max(0, (time.monotonic()-(offer_start+tick*.1))*1000)
                for i, f in enumerate(logs):
                    rows = []
                    for j in range(per_tick):
                        tag = f'{i:02}:{tick:04}:{j:02}'
                        prefix = 'load-'+tag+' '
                        padding = ('R'*900 if (tick+j)%2 == 0 else base64.b85encode(hashlib.shake_256(f'{seed}:{tag}'.encode()).digest(720)).decode())
                        body = (prefix+padding)[:900]
                        rows.append((tag, body, hashlib.sha256(body.encode()).hexdigest()))
                    f.write(('\n'.join(row[1] for row in rows)+'\n').encode())
                    t = time.time_ns()
                    for tag, body, sha in rows:
                        expected[tag] = (sha, t, phase)
                        src.write(json.dumps([tag, sha, t, phase, late_ms])+'\n')
                    if observer and tick % 50 == 0 and i == (tick//50) % nodes_count:
                        observer.target(rows[0][0], rows[0][2], t, phase)
        offer_elapsed = time.monotonic()-offer_start
        stop.wait(180)
        if errors:
            raise RuntimeError('; '.join(errors))
        if observer:
            observer.stop()
        stop.set(); sampler.join(remaining(5, 20))
        for p in kids[1:]: p.send_signal(signal.SIGTERM)
        for p in kids[1:]: p.wait(timeout=remaining(30, 30))
        for t in readers: t.join(remaining(5, 20))
        if observer:
            observer.quiescent(api)
        server.send_signal(signal.SIGTERM); server.wait(timeout=remaining(30, 30))
        if any(p.returncode != 0 for p in kids):
            raise RuntimeError('nonzero graceful child exit')
        for f in logs: f.close()
        dump(samples_path := root/'resources.json', samples)
        # Decode outside timing and stream the potentially large durable replay.
        with open(root/'recovered.jsonl', 'wb') as output, open(root/'dump.err', 'wb') as err:
            decoder = subprocess.Popen([str(bins/'examples/server_dump'), str(conf), '--records'],
                                       stdout=subprocess.PIPE, stderr=err)
            kids.append(decoder)
            decoded_bytes = 0
            try:
                # The decoder has an absolute timeout, including blocking pipe reads.
                timer = threading.Timer(remaining(300, 30), decoder.kill)
                timer.start()
                while chunk := decoder.stdout.read(65536):
                    decoded_bytes += len(chunk)
                    if decoded_bytes > 3*2**30:
                        decoder.kill()
                        raise RuntimeError('replay output exceeds 3GiB; partial replay preserved')
                    output.write(chunk)
                    if decoded_bytes % (16*2**20) < len(chunk):
                        if footprint(Path(os.environ['FABRIC_SCRATCH_ROOT'])) > 8*2**30 or shutil.disk_usage(root).free < 16*2**30:
                            decoder.kill()
                            raise RuntimeError('live scratch/free-disk bound reached during replay; partial replay preserved')
                if decoder.wait(timeout=remaining(10, 20)) != 0:
                    raise RuntimeError('replay decoder failed or reached absolute timeout')
            finally:
                timer.cancel()
                decoder.stdout.close()
            kids.remove(decoder)
        ack, retries, native_rtt = {}, 0, []
        max_log_backlog = 0
        for label, events in node_events.items():
            for e in events:
                if e['kind'] == 'cycle':
                    max_log_backlog = max(max_log_backlog, int(e['log_backlog_bytes']))
                elif e['status'] == 'ack':
                    ack[(label, int(e['sequence']))] = e
                    native_rtt.append(int(e['elapsed_us'])/1000)
                else: retries += 1
        seen, duplicates, gaps, batch_keys, recovered_hashes = {}, 0, [], set(), {}
        latencies = {phase: {'ingest': [], 'ack': [], 'source_to_ingest': []} for phase in ['normal','burst','recovery','all']}
        total_bytes = 0
        with open(root/'recovered.jsonl') as replay, gzip.open(root/'data-clocks.jsonl.gz', 'wt') as raw_clocks, gzip.open(root/'recovered-hashes.jsonl.gz','wt') as hashout:
            for line in replay:
                r = json.loads(line); raw = base64.b64decode(r['bytes']); total_bytes += len(raw)
                b = query_oracle.decode_batch(raw)
                key = (r['label'], b['sequence']); sha = hashlib.sha256(raw).hexdigest()
                if key in batch_keys: raise RuntimeError('duplicate recovered Batch')
                batch_keys.add(key); recovered_hashes[key] = sha; gaps.extend(b['gaps'])
                hashout.write(json.dumps([*key, sha, len(raw), r['received_ns']])+'\n')
                for log in query_oracle.decode_logs_request(b['logs_bytes']):
                    body = log['body']; tag = body.split(' ',1)[0].removeprefix('load-')
                    if tag in seen: duplicates += 1
                    seen[tag] = hashlib.sha256(body.encode()).hexdigest()
                    if tag not in expected: continue
                    _, source_ns, phase = expected[tag]
                    collected = log['observed_time_unix_nano']; received = r['received_ns']; a = ack.get(key)
                    observed_ack = int(a['t']) if a else None
                    raw_clocks.write(json.dumps([tag, r['label'], b['sequence'], source_ns, collected, received, observed_ack])+'\n')
                    for group in [phase, 'all']:
                        latencies[group]['ingest'].append((received-collected)/1e6)
                        latencies[group]['source_to_ingest'].append((received-source_ns)/1e6)
                        if observed_ack is not None: latencies[group]['ack'].append((observed_ack-collected)/1e6)
        checks = compare(expected, seen)
        missing_acks = sum(key not in recovered_hashes or recovered_hashes[key] != e['sha256'] for key,e in ack.items())
        by_node = {}
        for label, seq in batch_keys: by_node.setdefault(label, []).append(seq)
        contiguous = all(sorted(v) == list(range(1, max(v)+1)) for v in by_node.values())
        offsets = [(x['wall_ns']-x['mono_ns'])/1e6 for x in samples]
        server_peak = max(x['server']['hwm_kib'] for x in samples)/1024
        node_peak = max(n['hwm_kib'] for x in samples for n in x['nodes'])/1024
        measured_wall = (samples[-1]['mono_ns']-samples[0]['mono_ns'])/1e9
        server_cpu = samples[-1]['server']['cpu_s']-samples[0]['server']['cpu_s']
        node_cpu = sum(n['cpu_s'] for n in samples[-1]['nodes'])-sum(n['cpu_s'] for n in samples[0]['nodes'])
        latency_stats = {phase: {kind: percentile(vals) for kind, vals in populations.items()} for phase,populations in latencies.items()}
        gates = {'exact_source_logs': not any(checks.values()) and duplicates==0 and len(seen)==len(expected)==1_260_000,
                 'no_collection_gaps': not gaps, 'ack_hash_recovery': missing_acks==0 and len(ack)==len(batch_keys),
                 'contiguous_recovered_sequences': contiguous, 'clean_child_exits': all(p.returncode==0 for p in kids),
                 'server_rss_le_4gb': server_peak * 2**20 <= 4_000_000_000}
        diagnostics = {
            'collection_to_ingest_p99_le_1s': latency_stats['all']['ingest']['p99'] is not None and latency_stats['all']['ingest']['p99'] <= 1000 and latency_stats['all']['ingest']['negative']==0,
            'collection_to_ack_p99_le_1s': latency_stats['all']['ack']['p99'] is not None and latency_stats['all']['ack']['p99'] <= 1000 and latency_stats['all']['ack']['samples']==len(expected) and latency_stats['all']['ack']['negative']==0,
            'server_rss_le_2gib': server_peak <= 2048, 'node_rss_le_64mib': node_peak <= 64,
            'clock_offset_range_le_5ms': max(offsets)-min(offsets) <= 5}
        summary = {'deployment': name, 'nodes': nodes_count, 'expected_logs': len(expected), 'recovered_logs': len(seen),
                   'comparison': checks, 'duplicates': duplicates, 'collection_gaps': gaps,
                   'latency_ms': latency_stats, 'native_attempt_rtt_ms': percentile(native_rtt),
                   'acknowledged_batches': len(ack), 'recovered_batches': len(batch_keys), 'encoded_batch_bytes': total_bytes,
                   'encoded_bytes_per_source_log_including_metrics': total_bytes/len(expected),
                   'offer_window_seconds': offer_elapsed, 'mean_encoded_mib_s_offer_window': total_bytes/2**20/offer_elapsed,
                   'server_peak_rss_mib': server_peak, 'node_peak_rss_mib': node_peak,
                   'aggregate_nodes_peak_rss_mib': max(sum(n['rss_kib'] for n in x['nodes']) for x in samples)/1024,
                   'measured_wall_seconds': measured_wall, 'server_cpu_seconds': server_cpu, 'nodes_cpu_seconds': node_cpu,
                   'server_mean_cpu_equivalents': server_cpu/measured_wall, 'nodes_mean_cpu_equivalents': node_cpu/measured_wall,
                   'max_sealed_files_waiting': max(x['sealed_files'] for x in samples),
                   'segments_final': samples[-1]['segments'], 'journal_final_bytes': samples[-1]['journal_bytes'],
                   'peak_trial_disk_bytes': max(x['state_bytes'] for x in samples), 'max_node_log_backlog_bytes': max_log_backlog,
                   'non_ack_attempts': retries, 'clock_offset_range_ms': max(offsets)-min(offsets),
                   'seed': seed, 'diagnostics': diagnostics, 'gates': gates, 'passed': all(gates.values()), 'child_exits': [p.returncode for p in kids]}
        dump(root/'summary.json', summary)
        if observer:
            observer.finish(root, summary, samples, node_events)
        # Replay stays until wrapper child-resource gates and exact preservation succeed.
        print(json.dumps({'deployment': name, 'passed': summary['passed'], 'latency_ms': latency_stats['all'], 'server_rss_mib': server_peak}), flush=True)
        return summary
    finally:
        stop.set()
        dump(root/'resources.json', samples)
        if observer:
            try:
                observer.stop()
            except Exception as exc:
                dump(root/'consumer-cleanup-error.json', {'error':repr(exc)})
        for p in kids:
            if p.poll() is None: p.kill(); p.wait(timeout=max(.01, min(10, deadline-time.monotonic())))
        for t in readers:
            t.join(max(.01, min(5, deadline-time.monotonic())))
        if 'sampler' in locals():
            sampler.join(max(.01, min(5, deadline-time.monotonic())))
        for f in logs:
            if not f.closed: f.close()


if __name__ == '__main__':
    if len(sys.argv) < 4 or sys.argv[1] != '--enter':
        raise SystemExit('native.py is an internal cgroup exec wrapper')
    cgroups.enter(Path(sys.argv[2]))
    os.execvp(sys.argv[3], sys.argv[3:])
