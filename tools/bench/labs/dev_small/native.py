#!/usr/bin/env python3
"""Private initial-screen native adapter; execution belongs to the coordinator."""
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


def custody_mismatches(expected_hashes, recovered_hashes):
    return sum(recovered_hashes.get(key) != sha for key,sha in expected_hashes.items())


def controls():
    expected = {'a': ('same', 0, 'history')}
    cases = {'identical': {'a':'same'}, 'missing': {}, 'changed': {'a':'altered'}, 'unexpected': {'a':'same','x':'extra'}}
    results = {name:compare(expected,seen) for name,seen in cases.items()}
    if any(results['identical'].values()) or not all(any(results[k].values()) for k in ('missing','changed','unexpected')):
        raise RuntimeError('source/seed comparator controls failed')
    seed = {('oldhistory',1):'batch-sha'}
    altered = {('oldhistory',1):'altered-batch-sha'}
    if custody_mismatches(seed,seed) != 0 or custody_mismatches(seed,altered) != 1 or custody_mismatches(seed,{}) != 1:
        raise RuntimeError('seed ACK control failed')
    return {'source_seed_comparator':results,'altered_seed_batch_hash_rejected':True,'missing_seed_batch_rejected':True}


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


def trial(root, bins, name, server_cpus, node_cpus, observer=None, query_plan=None, *,
          nodes_count=None, rates=None, history="H0", near_rotation=False,
          schedule=(60,60,60), settle_s=5, drain_s=20):
    root.mkdir()
    (root / 'owned').write_text('dev-small initial-screen owned cell\n')
    nodes_count = nodes_count if nodes_count is not None else (1 if name == 'development' else 20)
    rates = tuple(rates or ((10,30,10) if nodes_count == 1 else (1000,3000,1000)))
    if nodes_count < 1 or len(rates) != 3 or len(schedule) != 3 or any(r <= 0 for r in rates) or any(d <= 0 for d in schedule):
        raise ValueError('positive nodes, three rates and three phase durations required')
    if history not in ('H0','H256') or (near_rotation and history != 'H0'):
        raise ValueError('history must be H0/H256; nearrotation is separate from H256')
    phase_end_ticks = [round(sum(schedule[:i+1])*10) for i in range(3)]
    accumulators = [0]*nodes_count
    make_certs(root)
    port = free_port()
    admin = os.urandom(32).hex()
    (root / 'admin-token').write_text(admin+'\n')
    conf = root / 'server.conf'
    conf.write_text(f'listen=127.0.0.1:{port}\ntls_cert={root}/server.pem\ntls_key={root}/server.key\nstate_dir={root}/state\nadmin_token_file={root}/admin-token\njournal_bytes=1073741824\njournal_file_bytes=67108864\nretention_s=86400\nretention_bytes=1073741824\nseal_workers=1\n')
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

    def spawn(binary, arguments, cpus, limit, label, piped=False):
        p = subprocess.Popen(['prlimit', f'--as={limit}', '--', 'taskset', '-c', ','.join(map(str, cpus)), str(binary), *map(str, arguments)],
                             stdout=subprocess.PIPE if piped else open(root/(label+'.out'), 'wb'), stderr=open(root/(label+'.err'), 'wb'), bufsize=0)
        kids.append(p)
        return p

    def api(endpoint, body):
        req = urllib.request.Request(f'https://127.0.0.1:{port}'+endpoint, data=json.dumps(body).encode(),
            headers={'authorization': 'Bearer '+admin, 'content-type': 'application/json'}, method='POST')
        with urllib.request.urlopen(req, context=ctx, timeout=10) as r:
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
        seed_start = time.monotonic()
        condition = 'nearrotation' if near_rotation else history
        with open(root/'seed-summary.json','wb') as output, open(root/'seed.err','wb') as error:
            subprocess.run(['prlimit','--as=4294967296','--','taskset','-c',','.join(map(str,server_cpus)),
                str(bins/'examples/lab_history_seed'), str(conf), condition, str(root/'seed-ledger.jsonl')],
                stdout=output, stderr=error, check=True, timeout=450)
        seed_summary = json.loads((root/'seed-summary.json').read_text())
        seed_hashes = {}
        with (root/'seed-ledger.jsonl').open() as ledger:
            for line in ledger:
                batch = json.loads(line)
                seed_hashes[(batch['label'], batch['sequence'])] = batch['sha256']
                for tag, sha in batch['source']:
                    if tag in expected: raise RuntimeError('duplicate seed source tag')
                    expected[tag] = (sha, None, 'history')
        if history == 'H256' and (root/'state/journal/batches.faj').stat().st_size != 0:
            raise RuntimeError('H256 must start with empty active journal')
        seed_summary['preparation_seconds'] = time.monotonic()-seed_start
        seed_summary['active_journal_bytes'] = (root/'state/journal/batches.faj').stat().st_size
        if near_rotation and not 64*2**20-256*2**10 <= seed_summary['active_journal_bytes'] < 64*2**20:
            raise RuntimeError('nearrotation active framing outside frozen 256KiB margin')
        dump(root/'seed-summary.json',seed_summary)
        dump(root/'seed-verification.json', {'prefix_sha256':seed_summary['prefix_sha256'], 'seed_replay_exact':seed_summary['seed_replay_exact'], 'seed_batches':len(seed_hashes), 'seed_source_rows':seed_summary['rows'], 'source_body_fidelity':'graded_against_final_recovery', 'controls':controls()})
        if observer and hasattr(observer,'seed_info'): observer.seed_info(root,seed_summary)
        server = spawn(bins/'fabric-server', ['serve', conf], server_cpus, 4*2**30, 'server')
        for _ in range(200):
            if server.poll() is not None:
                raise RuntimeError('server exited during startup')
            try:
                with socket.create_connection(('127.0.0.1', port), timeout=.1):
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
            p = spawn(bins/'fabric-node', ['run', nconf], node_cpus, 512*2**20, label, True)
            thread = threading.Thread(target=read_node, args=(p, label)); thread.start(); readers.append(thread)

        def sample():
            while not stop.is_set():
                try:
                    if time.monotonic()-started > 900:
                        raise RuntimeError('900 s experiment limit')
                    if any(p.poll() is not None for p in kids):
                        raise RuntimeError('child exited before shutdown')
                    size = footprint(root)
                    if size > 4*2**30 or shutil.disk_usage(root).free < 4*2**30:
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
                    samples.append(row)
                except Exception as e:
                    errors.append(str(e)); stop.set(); return
                stop.wait(1)
        sampler = threading.Thread(target=sample); sampler.start()
        # Startup wait remains bounded and interruptible.
        stop.wait(settle_s)
        offer_start = time.monotonic()
        if observer:
            observer.start(api, time.time_ns(), nodes_count)
        with gzip.open(root/'sources.jsonl.gz', 'wt') as src:
            for tick in range(phase_end_ticks[-1]):
                stop.wait(max(0, offer_start+tick*.1-time.monotonic()))
                if stop.is_set():
                    raise RuntimeError('; '.join(errors))
                phase_index = 0 if tick < phase_end_ticks[0] else 1 if tick < phase_end_ticks[1] else 2
                phase = ('normal','burst','recovery')[phase_index]
                late_ms = max(0, (time.monotonic()-(offer_start+tick*.1))*1000)
                for i, f in enumerate(logs):
                    rows = []
                    accumulators[i] += rates[phase_index]
                    count, accumulators[i] = divmod(accumulators[i], 10*nodes_count)
                    for j in range(count):
                        tag = f'{i:02}:{tick:04}:{j:02}'
                        prefix = 'load-'+tag+' '
                        padding = ('R'*900 if (tick+j)%2 == 0 else base64.b85encode(hashlib.shake_256(f'2703163393:{tag}'.encode()).digest(720)).decode())
                        body = (prefix+padding)[:900]
                        rows.append((tag, body, hashlib.sha256(body.encode()).hexdigest()))
                    if rows: f.write(('\n'.join(row[1] for row in rows)+'\n').encode())
                    t = time.time_ns()
                    for tag, body, sha in rows:
                        expected[tag] = (sha, t, phase)
                        src.write(json.dumps([tag, sha, t, phase, late_ms])+'\n')
                    if observer and rows and i == (tick//50) % nodes_count and (observer.select_visibility_target(t,tick) if hasattr(observer,'select_visibility_target') else tick % 50 == 0):
                        observer.target(rows[0][0], rows[0][2], t, phase)
        offer_elapsed = time.monotonic()-offer_start
        stop.wait(drain_s)
        if errors:
            raise RuntimeError('; '.join(errors))
        if observer:
            observer.stop()
        stop.set(); sampler.join(5)
        for p in kids[1:]: p.send_signal(signal.SIGTERM)
        for p in kids[1:]: p.wait(timeout=30)
        for t in readers: t.join(5)
        if observer:
            observer.quiescent(api)
        server.send_signal(signal.SIGTERM); server.wait(timeout=30)
        if any(p.returncode != 0 for p in kids):
            raise RuntimeError('nonzero graceful child exit')
        for f in logs: f.close()
        dump(samples_path := root/'resources.json', samples)
        # Decode outside timing and stream the potentially large durable replay.
        with open(root/'recovered.jsonl', 'wb') as output:
            subprocess.run([str(bins/'examples/server_dump'), str(conf), '--records'], stdout=output,
                           stderr=open(root/'dump.err','wb'), check=True, timeout=180)
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
                    body = log['body']; tag = body.split(' ',1)[0]
                    if tag.startswith('load-'): tag = tag.removeprefix('load-')
                    if tag in seen: duplicates += 1
                    seen[tag] = hashlib.sha256(body.encode()).hexdigest()
                    if tag not in expected: continue
                    _, source_ns, phase = expected[tag]
                    collected = log['observed_time_unix_nano']; received = r['received_ns']; a = ack.get(key)
                    observed_ack = int(a['t']) if a else None
                    raw_clocks.write(json.dumps([tag, r['label'], b['sequence'], source_ns, collected, received, observed_ack, seen[tag]])+'\n')
                    if phase == 'history': continue
                    for group in [phase, 'all']:
                        latencies[group]['ingest'].append((received-collected)/1e6)
                        latencies[group]['source_to_ingest'].append((received-source_ns)/1e6)
                        if observed_ack is not None: latencies[group]['ack'].append((observed_ack-collected)/1e6)
        checks = compare(expected, seen)
        missing_acks = sum(key not in recovered_hashes or recovered_hashes[key] != e['sha256'] for key,e in ack.items())
        seed_mismatch = custody_mismatches(seed_hashes,recovered_hashes)
        if set(ack) & set(seed_hashes): raise RuntimeError('seed/live custody namespaces overlap')
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
        gates = {'exact_source_logs': not any(checks.values()) and duplicates==0 and len(seen)==len(expected),
                 'no_collection_gaps': not gaps, 'ack_hash_recovery': missing_acks==0 and seed_mismatch==0 and len(ack)+len(seed_hashes)==len(batch_keys),
                 'contiguous_recovered_sequences': contiguous, 'no_unexpected_retries': retries == 0, 'clean_child_exits': all(p.returncode==0 for p in kids),
                 'collection_to_ingest_p99_le_1s': latency_stats['all']['ingest']['p99'] <= 1000 and latency_stats['all']['ingest']['negative']==0,
                 'collection_to_ack_p99_le_1s': latency_stats['all']['ack']['p99'] <= 1000 and latency_stats['all']['ack']['samples']==len(expected)-seed_summary['rows'] and latency_stats['all']['ack']['negative']==0,
                 'server_rss_le_2gib': server_peak <= 2048, 'node_rss_le_64mib': node_peak <= 64,
                 'clock_offset_range_le_5ms': max(offsets)-min(offsets) <= 5}
        seed_summary['seed_batch_hash_mismatches'] = seed_mismatch
        summary = {'deployment': name, 'nodes': nodes_count, 'offered_rates':rates, 'schedule_seconds':schedule, 'history':seed_summary, 'expected_logs': len(expected), 'recovered_logs': len(seen),
                   'comparison': checks, 'duplicates': duplicates, 'collection_gaps': gaps,
                   'latency_ms': latency_stats, 'native_attempt_rtt_ms': percentile(native_rtt),
                   'acknowledged_batches': len(ack), 'recovered_batches': len(batch_keys), 'encoded_batch_bytes': total_bytes,
                   'encoded_bytes_per_source_log_including_metrics': (total_bytes-seed_summary['encoded_bytes'])/(len(expected)-seed_summary['rows']),
                   'offer_window_seconds': offer_elapsed, 'mean_encoded_mib_s_offer_window': (total_bytes-seed_summary['encoded_bytes'])/2**20/offer_elapsed,
                   'server_peak_rss_mib': server_peak, 'node_peak_rss_mib': node_peak,
                   'aggregate_nodes_peak_rss_mib': max(sum(n['rss_kib'] for n in x['nodes']) for x in samples)/1024,
                   'measured_wall_seconds': measured_wall, 'server_cpu_seconds': server_cpu, 'nodes_cpu_seconds': node_cpu,
                   'server_mean_cpu_equivalents': server_cpu/measured_wall, 'nodes_mean_cpu_equivalents': node_cpu/measured_wall,
                   'max_sealed_files_waiting': max(x['sealed_files'] for x in samples),
                   'segments_final': samples[-1]['segments'], 'journal_final_bytes': samples[-1]['journal_bytes'],
                   'peak_trial_disk_bytes': max(x['state_bytes'] for x in samples), 'max_node_log_backlog_bytes': max_log_backlog,
                   'non_ack_attempts': retries, 'clock_offset_range_ms': max(offsets)-min(offsets),
                   'gates': gates, 'passed': all(gates.values()), 'child_exits': [p.returncode for p in kids]}
        dump(root/'summary.json', summary)
        dump(root/'seed-verification.json', {'prefix_sha256':seed_summary['prefix_sha256'], 'seed_replay_exact':seed_summary['seed_replay_exact'], 'seed_batches':len(seed_hashes), 'seed_source_rows':seed_summary['rows'], 'seed_batch_hash_mismatches':seed_mismatch, 'seed_body_fidelity_in_exact_source_gate':gates['exact_source_logs'], 'controls':controls()})
        if observer:
            observer.finish(root, summary, samples, node_events)
        (root/'recovered.jsonl').unlink()  # owned decoded replay; preserve hashes/data clocks instead
        print(json.dumps({'deployment': name, 'passed': summary['passed'], 'latency_ms': latency_stats['all'], 'server_rss_mib': server_peak}), flush=True)
        return summary
    finally:
        stop.set()
        if observer:
            observer.stop()
        for p in kids:
            if p.poll() is None: p.kill(); p.wait(timeout=10)
        for t in readers:
            t.join(5)
        if 'sampler' in locals():
            sampler.join(5)
        for f in logs:
            if not f.closed: f.close()


if __name__ == '__main__':
    raise SystemExit('Use the dev_small coordinator under tools/resource_group.py')
