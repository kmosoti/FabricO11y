"""Six production-access main cells; shortened --smoke is never qualification."""
from __future__ import annotations
import argparse
import base64
import collections
import gzip
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import threading
import time
import urllib.request

from release_runtime import CandidateFixture, process_kib
from release_fixture import traces, NS
from release_timing import native, population, p99
from workload import SEEDS, entropy_body
import delivery_oracle
import query_oracle
import soak_companion


def records(path):
    with Path(path).open() as source:
        return [json.loads(line) for line in source if line.strip()]


def key(record):
    return record['node_id'], record['generation'], record['sequence']


def parse_fields(line):
    return dict(part.split('=', 1) for part in line.split()[1:])


def native_custody(raw, logfile):
    observation = soak_companion.inspect_records(raw)
    sources = {r['sequence']: r for r in raw[:-1]}
    ledger = list(observation['projected_sources'])
    for line in Path(logfile).read_text().splitlines():
        if not line.startswith('delivery '):
            continue
        event = parse_fields(line)
        source = sources[int(event['sequence'])]
        payload = base64.b64decode(source['bytes'], validate=True)
        if hashlib.sha256(payload).hexdigest() != event['sha256']:
            raise ValueError('attempt differs from independently retained source')
        ident = {name: source[name] for name in ('node_id', 'generation', 'sequence')}
        ledger.append({'type': 'attempt', **ident, 'injected_conflict': False,
                       'bytes': soak_companion.project(source)['bytes']})
        status = event['status']
        status = 'unauthorized' if status == 'forbidden' else status
        response = {'type': 'response', **ident, 'kind': status}
        if status == 'ack':
            response['committed_through'] = int(event['committed_through'])
        ledger.append(response)
    ledger.append(raw[-1])
    return observation, ledger


def run(args):
    seconds, warmup = (8, 2) if args.smoke else (135, 15)
    summary = {'classification': 'disposable smoke' if args.smoke else 'registered main cell',
               'seed': args.seed, 'tier': args.tier, 'seconds': seconds, 'passed': False,
               'backlog_rule': args.backlog_rule}
    with CandidateFixture(args.deb_receipt, args.rpm_receipt, args.out) as fixture:
        summary_path = fixture.out / 'summary.json'
        try:
            perform(fixture, args, seconds, warmup, summary)
            fixture.receipt['passed'] = summary['passed']
        except BaseException as error:
            summary['failure_type'] = type(error).__name__
            summary['failure'] = str(error)[:1200]
            raise
        finally:
            summary_path.write_text(json.dumps(summary, indent=2) + '\n')
    return 0 if summary['passed'] else 1


def perform(f, args, seconds, warmup, summary):
    enrolled = [f.enroll(f'sim{i:04d}') for i in range(args.tier)]
    source_log = f.work / 'source.log'; source_log.touch()
    edge_enrollment = f.enroll('edge', [source_log])
    reader = f.reader([x['enrollment_id'] for x in enrolled] + [edge_enrollment['enrollment_id']])
    tokens = f.work / 'sim-tokens'
    tokens.write_text(''.join(e['token'] + '\n' for e in enrolled)); tokens.chmod(0o600)
    token = f.work / 'edge-token'
    token.write_text(edge_enrollment['token'] + '\n'); token.chmod(0o600)
    # Reserve a fresh loopback trace listener without borrowing host services.
    from delivery_faults import free_port
    trace_port = free_port()
    edge_config = f.work / 'edge.conf'
    edge_config.write_text(f'spool_dir={f.work}/edge-spool\nspool_bytes=268435456\n'
            f'metric_interval_s=15\nlog={source_log}\nserver_url={f.origin}\n'
            f'server_ca={f.work}/ca.pem\ntoken_file={token}\ntraces_listen=127.0.0.1:{trace_port}\n')
    edge = f.spawn([f.bins / 'fabric-node', 'run', edge_config, '--timing-events'], 'edge', f.edge_group)
    import socket
    deadline = time.monotonic() + 15
    while True:
        try:
            with socket.create_connection(('127.0.0.1', trace_port), timeout=.2):
                break
        except OSError:
            if time.monotonic() > deadline or edge.poll() is not None:
                raise RuntimeError('native edge OTLP listener unavailable')
            time.sleep(.1)
    # Count actual visible PWA traffic separately from benchmark reads.
    f.bridge._js("window.releaseUi={queries:0,failed:0};const original=window.fetch;window.fetch=function(...args){const q=new URL(typeof args[0]==='string'?args[0]:args[0].url,location.href).pathname==='/v1/console/query';if(q)window.releaseUi.queries++;return original(...args).then(r=>{if(q&&r.status!==200)window.releaseUi.failed++;return r;},e=>{if(q&&e.name!=='AbortError')window.releaseUi.failed++;throw e;});}")
    f.bridge.poll_ui(True)
    offers = {'logs': [], 'traces': []}
    failures, queries, samples, clocks, puts = [], [], [], [], {}
    begin_mono, begin_wall = time.monotonic_ns(), time.time_ns()
    finished = threading.Event()
    writer_log = (f.work / 'offers.jsonl').open('w')
    write_lock = threading.Lock()

    def offered(record):
        with write_lock:
            writer_log.write(json.dumps(record) + '\n'); writer_log.flush()

    def log_writer():
        try:
            with source_log.open('ab', buffering=0) as sink:
                for tick in range(seconds * 2):
                    time.sleep(max(0, (begin_mono + tick * NS // 2 - time.monotonic_ns()) / NS))
                    body = 'R' * 512 if tick % 2 == 0 else entropy_body(args.seed, args.tier, tick)
                    row = {'kind': 'log', 'tick': tick, 'offset': tick * 513, 'body': body,
                           'offered_mono_ns': time.monotonic_ns(), 'offered_unix_ns': time.time_ns()}
                    offers['logs'].append(row); offered(row)
                    sink.write((body + '\n').encode())
        except BaseException as error:
            failures.append('log writer: ' + repr(error))

    def trace_writer():
        try:
            for tick in range(seconds * 10):
                time.sleep(max(0, (begin_mono + tick * NS // 10 - time.monotonic_ns()) / NS))
                wall, mono = time.time_ns(), time.monotonic_ns()
                payload, rows = traces(args.seed, tick, wall)
                row = {'kind': 'trace', 'tick': tick, 'offered_mono_ns': mono,
                       'offered_unix_ns': wall, 'rows': rows}
                offers['traces'].append(row); offered(row)
                request = urllib.request.Request(f'http://127.0.0.1:{trace_port}/v1/traces', data=payload,
                                                 headers={'content-type': 'application/x-protobuf'})
                with urllib.request.urlopen(request, timeout=3) as response:
                    if response.status != 200:
                        raise RuntimeError('native trace intake rejected an offered export')
                    response.read(65536)
        except BaseException as error:
            failures.append('trace writer: ' + repr(error))

    def prober():
        try:
            for second in range(seconds):
                time.sleep(max(0, (begin_mono + second * NS - time.monotonic_ns()) / NS))
                wall = time.time_ns()
                for kind, node in [('logs', f'sim{second % args.tier:04d}'), ('logs', 'edge'), ('spans', 'edge')]:
                    body = {'kind': kind, 'node': node, 'from_ns': wall - 5 * NS,
                            'to_ns': wall + NS, 'limit': 1000}
                    page = reader.query(body)
                    queries.append({'second': second, 'kind': kind, 'node': node, 'query': body,
                                    'received_mono_ns': time.monotonic_ns(), 'received_unix_ns': time.time_ns(),
                                    'answer': page})
                    if not page.get('complete') or page.get('next_page'):
                        raise RuntimeError('main visibility probe incomplete or unexpectedly paginated')
        except BaseException as error:
            failures.append('prober: ' + repr(error))
        finally:
            finished.set()

    def controller():
        try:
            for i in range(args.tier):
                puts[i] = time.time_ns()
                status, _ = f.bridge.request(f'/v1/console/nodes/sim{i:04d}/config', 'PUT',
                                             {'logs': [], 'metric_interval_s': 30})
                if status != 200:
                    raise RuntimeError('production configuration update failed')
                # One operator credential remains subject to normal admission.
                # Pace changes independently of UI/resourcing/probe schedules.
                time.sleep(.15)
        except BaseException as error:
            failures.append('controller: ' + repr(error))

    threads = [threading.Thread(target=fn, daemon=True) for fn in (log_writer, trace_writer, prober)]
    sim = f.spawn([f.helpers / 'spindle_sim', '--server-url', f.origin, '--ca', f.work / 'ca.pem',
                   '--tokens', tokens, '--seed', hex(args.seed), '--seconds', str(seconds),
                   '--workers', str(args.tier), '--out', f.work / 'sim'], 'sim')
    for thread in threads:
        thread.start()
    configured = paused = resumed = False
    stop = begin_mono + (seconds + 120) * NS
    while time.monotonic_ns() < stop:
        now = time.monotonic_ns()
        clocks.append({'mono_ns': now, 'unix_ns': time.time_ns()})
        sample = f.resource_sample()
        sample['edge_rss_kib'] = process_kib(edge.pid)
        sample['edge_hwm_kib'] = process_kib(edge.pid, 'VmHWM')
        samples.append(sample)
        elapsed = (now - begin_mono) / NS
        if not configured and elapsed >= (3 if args.smoke else warmup + 45):
            configured = True
            control = threading.Thread(target=controller, daemon=True)
            threads.append(control)
            control.start()
        if not args.smoke and not paused and elapsed >= warmup + 40:
            f.bridge.poll_ui(False); paused = True
        if not args.smoke and paused and not resumed and elapsed >= warmup + 50:
            f.bridge.poll_ui(True); resumed = True
        if sim.poll() is not None and finished.is_set() and all(not t.is_alive() for t in threads):
            break
        if failures:
            raise RuntimeError(failures[0])
        time.sleep(.5)
    else:
        raise RuntimeError('main cell did not complete and drain within deadline')
    for thread in threads:
        thread.join(timeout=5)
    writer_log.close()
    if failures or sim.returncode != 0:
        raise RuntimeError('producer or simulator failure: ' + repr(failures))
    # Allow the native tail to collect, then require its durable pending cursor
    # to drain. Inspection is bounded and does not become a source oracle.
    from outage_drain import inspect
    drain_limit = time.monotonic() + 120
    while True:
        view = inspect(f.bins / 'fabricctl', edge_config)
        if int(view['acked_through']) == int(view['next_sequence']) - 1 and int(view['log_records']) >= len(offers['logs']):
            break
        if time.monotonic() > drain_limit:
            raise RuntimeError('native edge did not drain')
        time.sleep(.5)
    ui = f.bridge._js('return window.releaseUi')
    f.bridge.poll_ui(False)
    edge.send_signal(signal.SIGTERM)
    if edge.wait(timeout=20) != 0:
        raise RuntimeError('native edge shutdown failed')
    final_resource = f.resource_sample()
    f.stop_server()
    # All original source prefixes must still exist. Never reconstruct missing
    # native sources from server recovery or infer ACKs from row counts.
    native_sets, ledgers, timings = {}, [], {}
    for name, config, output in [('edge', edge_config, f.out / 'edge.out'),
            ('companion', f.work / 'state/self-spindle/node.conf', f.out / 'server-0.out')]:
        path = f.work / (name + '-custody')
        observation = soak_companion.dump_stopped_spool(f.helpers / 'spool_dump', config, path,
                                                       processes_stopped=True)
        raw = records(path / 'spool.jsonl')
        same, ledger = native_custody(raw, output)
        if same != observation:
            raise RuntimeError('native source observations disagree')
        native_sets[name] = {'observation': observation, 'raw': raw}
        ledgers.extend(ledger)
        timings[name] = native(output, raw[:-1])
    recovered_file = f.work / 'recovered.jsonl'
    with recovered_file.open('wb') as output:
        subprocess.run([str(f.helpers / 'server_dump'), str(f.config)], stdout=output,
                       stderr=subprocess.PIPE, check=True, timeout=120)
    recovered = records(recovered_file)
    recovered_sizes = {key(r): len(base64.b64decode(r['bytes'])) for r in recovered}
    projected = [soak_companion.project(r) for r in recovered]
    source_ledger = records(f.work / 'sim/transcript.jsonl')
    combined = source_ledger + ledgers + projected + [{'type': 'end'}]
    verdict = delivery_oracle.check(json.dumps(r) for r in combined)
    native_recovery = {name: soak_companion.validate_recovery(value['observation'], projected)
                       for name, value in native_sets.items()}
    events = records(f.work / 'sim/events.jsonl')
    sim_info = json.loads((f.work / 'sim/sim-summary.json').read_text())
    made, acked, applied = {}, {}, {}
    # Simulator random IDs bind deterministically to enrolled labels in recovery
    # metadata, which is checked against the independent source SHA transcript.
    record_file = f.work / 'records.jsonl'
    with record_file.open('wb') as output:
        subprocess.run([str(f.helpers / 'server_dump'), str(f.config), '--records'], stdout=output,
                       stderr=subprocess.PIPE, check=True, timeout=120)
    all_records = records(record_file)
    node_ids = {}
    for record in all_records:
        batch = query_oracle.decode_batch(base64.b64decode(record['bytes']))
        node_ids[record['label']] = batch['node_id'].hex()
    for event in events:
        if event['e'] == 'created':
            made[(event['id'], event['seq'])] = event['t']
        elif event['e'] == 'attempt' and event['kind'] == 'ack':
            acked.setdefault((event['id'], event['seq']), event['end'])
        elif event['e'] == 'applied' and event['id'] in puts and event['t'] >= puts[event['id']]:
            applied.setdefault(event['id'], event['t'])
    sim_entries = [{'key': [node_ids[f'sim{i:04d}'], 1, seq], 'created_ns': stamp,
                    'ack_ns': acked.get((i, seq)), 'bytes': recovered_sizes[(node_ids[f'sim{i:04d}'], 1, seq)]}
                   for (i, seq), stamp in made.items()]
    measure_wall = sim_info['began_unix_ns'] + warmup * NS
    timing = {'simulator': population(sim_entries, measure_wall,
              sim_info['began_unix_ns'] + seconds * NS, rule=args.backlog_rule)}
    for name, entries in timings.items():
        timing[name] = population(entries, begin_mono + warmup * NS,
                                 begin_mono + seconds * NS, rule=args.backlog_rule)
    # Independent source-body/offset and trace-parent checks, including every
    # offered item, use native Spool decoding; server bytes already matched above.
    edge_rows = [query_oracle.materialize_record('edge', 1, base64.b64decode(r['bytes']))
                 for r in native_sets['edge']['raw'][:-1]]
    lines = [row for m in edge_rows for row in m.log_rows if row['attributes'].get('log.file.path') == str(source_log)]
    log_exact = sorted((int(r['attributes']['log.file.offset.start']), r['body']) for r in lines) == sorted((r['offset'], r['body']) for r in offers['logs'])
    trace_fields = ['trace_id', 'span_id', 'parent_span_id', 'name', 'start_ns', 'end_ns', 'kind', 'status', 'attributes']
    wanted = [json.dumps({k: r[k] for k in trace_fields}, sort_keys=True) for offer in offers['traces'] for r in offer['rows']]
    actual = [json.dumps({k: r[k] for k in trace_fields}, sort_keys=True) for m in edge_rows for r in m.span_rows]
    trace_exact = collections.Counter(wanted) == collections.Counter(actual)
    visibility = grade_visibility(queries, offers, args.seed, args.tier, made, warmup, str(source_log))
    clock_offsets = [r['unix_ns'] - r['mono_ns'] for r in clocks]
    config_delays = [(applied[i] - stamp) / NS for i, stamp in puts.items() if i in applied]
    gates = {'delivery_oracle': verdict.passed, 'native_custody': all(r['passed'] for r in native_recovery.values()),
             'exact_native_logs': log_exact, 'exact_native_traces': trace_exact,
             'clock_mapping_le_1ms': max(clock_offsets) - min(clock_offsets) <= 1_000_000,
             'all_timing_backlog': all(r['passed'] for r in timing.values()),
             'visibility': all(v['passed'] for v in visibility.values()),
             'config_apply_le_30s': len(config_delays) == args.tier and max(config_delays) <= 30,
             'ui_active': ui['queries'] >= (1 if args.smoke else 20) and ui['failed'] == 0,
             'server_rss_le_2gib': final_resource['server_hwm_kib'] <= 2 * 1024**2,
             'edge_rss_le_64mib': max(s['edge_hwm_kib'] for s in samples) <= 64 * 1024,
             'no_oom': all('oom_kill 0' in v['memory.events'] for v in final_resource['groups'].values())}
    if args.smoke:
        # Short smoke validates mechanisms without applying minimum sample or
        # statistical release gates to an intentionally different duration.
        gates.pop('all_timing_backlog'); gates.pop('visibility'); gates.pop('config_apply_le_30s')
    summary.update(passed=all(gates.values()), gates=gates, timing=timing,
                   visibility=visibility, config_apply_s=config_delays, ui=ui,
                   independent_offers={'logs': len(offers['logs']), 'traces': len(offers['traces'])},
                   created_batches=len(made), acked_batches=len(acked), native_custody=native_recovery,
                   violations=[v.__dict__ for v in verdict.violations][:10],
                   clock_mapping_spread_ns=max(clock_offsets) - min(clock_offsets),
                   resources=samples, final_resource=final_resource)
    # Freeze compact synthetic observations and hashes; no config/token/private
    # access-state file is included. Failed raw work remains private for diagnosis.
    for source in [record_file, recovered_file, f.work / 'offers.jsonl', f.work / 'sim/events.jsonl',
                   f.work / 'sim/transcript.jsonl', *(f.work / (n + '-custody/spool.jsonl') for n in native_sets)]:
        label = str(source.relative_to(f.work)).replace('/', '-') + '.gz'
        with source.open('rb') as incoming, gzip.open(f.out / label, 'wb') as output:
            import shutil
            shutil.copyfileobj(incoming, output)
    with gzip.open(f.out / 'visibility-answers.json.gz', 'wt') as output:
        json.dump(queries, output)
    if sum(p.stat().st_size for p in f.out.rglob('*') if p.is_file()) > 50 * 1024**2:
        raise RuntimeError('final compact evidence exceeded 50 MiB')


def grade_visibility(queries, offers, seed, tier, created, warmup, source_path):
    seen = {'simulator': {}, 'edge_logs': {}, 'edge_traces': {}}
    missing = dict.fromkeys(seen, 0)
    logs = {r['offset']: r for r in offers['logs']}
    trace = {r['rows'][0]['trace_id']: r for r in offers['traces']}
    for query in queries:
        if query['second'] < warmup:
            continue
        candidates = []
        for row in query['answer']['rows']:
            if query['node'] != 'edge':
                i = int(query['node'][3:]); seq, index = row['sequence'], row['index']
                tick = (seq - 1) * 2 + index
                if index not in (0, 1) or row['body'] != ('R' * 512 if tick % 2 == 0 else entropy_body(seed, i, tick)):
                    raise ValueError('simulator visibility answer differs from independent offer')
                origin = created[(i, seq)]
                candidates.append((origin, 'simulator', (i, seq, index), (query['received_unix_ns'] - origin) / NS))
            elif query['kind'] == 'logs' and row['attributes'].get('log.file.path') == source_path:
                offset = int(row['attributes']['log.file.offset.start'])
                offer = logs[offset]
                if row['body'] != offer['body']:
                    raise ValueError('native log visibility answer differs from independent offer')
                candidates.append((offer['offered_mono_ns'], 'edge_logs', offset,
                                   (query['received_mono_ns'] - offer['offered_mono_ns']) / NS))
            elif query['kind'] == 'spans' and row['name'] == 'release-span-0':
                offer = trace[row['trace_id']]
                expected = offer['rows'][0]
                if any(row[k] != v for k, v in expected.items()):
                    raise ValueError('trace visibility answer differs from independent offer')
                candidates.append((offer['offered_mono_ns'], 'edge_traces', row['trace_id'],
                                   (query['received_mono_ns'] - offer['offered_mono_ns']) / NS))
        if candidates:
            _, name, ident, latency = max(candidates, key=lambda x: x[0])
            seen[name].setdefault(ident, latency)
        else:
            name = 'simulator' if query['node'] != 'edge' else ('edge_logs' if query['kind'] == 'logs' else 'edge_traces')
            missing[name] += 1
    result = {}
    for name, values in seen.items():
        latency = list(values.values()); percentile = p99(latency)
        result[name] = {'samples': len(latency), 'missing_probes': missing[name], 'p99_s': percentile,
                        'passed': len(latency) >= 100 and missing[name] == 0 and min(latency) >= 0 and percentile <= 5}
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--deb-receipt', type=Path, required=True)
    parser.add_argument('--rpm-receipt', type=Path, required=True)
    parser.add_argument('--backlog-rule', choices=('sampled-v1', 'clearing-v2'), default='sampled-v1')
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--tier', type=int, choices=(10, 100), required=True)
    parser.add_argument('--seed', type=lambda v: int(v, 0), choices=SEEDS, required=True)
    parser.add_argument('--smoke', action='store_true')
    return run(parser.parse_args())


if __name__ == '__main__':
    raise SystemExit(main())
