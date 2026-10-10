"""Production burst/rejection cells under service-pressure-protocol.md."""
import argparse
import base64
import gzip
import hashlib
import json
from pathlib import Path
import random
import shutil
import subprocess
import threading
import time
import urllib.error
import urllib.request

import delivery_oracle
from release_main import native_custody, records
from release_runtime import CandidateFixture, directory_bytes
from release_fixture import attribute, fixed, message
from release_main_source import reconstruct, simulator_batch
from release_timing import NS, p99
import soak_companion
from stress_tier import batch_bytes
from workload import SEEDS
from workload import entropy_body


def verify_offers(seed, identities, seconds, burst, events, ledger):
    """Check every pre-send hash against independently specified burst bytes."""
    creations = [e for e in events if e['e'] == 'created']
    times = {(e['id'], e['seq']): e['t'] for e in creations}
    expected = {(i, s) for i in range(identities) for s in range(1, seconds+1)}
    if len(creations) != len(times) or set(times) != expected:
        raise ValueError('incomplete or duplicate independent stress offer population')
    sources = [r for r in ledger if r['type'] == 'source']
    if any(r['generation'] != 1 for r in sources):
        raise ValueError('stress source generation differs from registered workload')
    _, mapping = reconstruct(seed, identities, 1,
        [e for e in creations if e['seq'] == 1], [r for r in sources if r['sequence'] == 1])
    hashes = {(r['node_id'], r['sequence']): base64.b64decode(r['bytes'], validate=True) for r in sources}
    if len(hashes) != len(sources) or set(hashes) != {(n, s) for n in mapping.values() for s in range(1, seconds+1)}:
        raise ValueError('incomplete or duplicate independent stress source hashes')
    for (index, sequence), stamp in times.items():
        node = mapping[index]
        raw = simulator_batch(seed, index, bytes.fromhex(node), sequence, stamp)
        if burst[0] <= sequence-1 < burst[1]:
            resource = message(1, attribute('host.name', f'sim-{index:04d}')) + message(1, attribute('service.name', 'fabric-node-sim'))
            def logs(factor):
                rows = []
                for half in range(2*factor):
                    tick = (sequence-1)*2+half if half < 2 else 1_000_000+(sequence-1)*100+half
                    body = 'R'*512 if tick % 2 == 0 else entropy_body(seed, index, tick)
                    rows.append(message(2, message(5, message(1, body.encode())) + fixed(11, stamp)))
                return message(6, message(1, message(1, resource) + message(2, b''.join(rows))))
            normal = logs(1)
            if not raw.endswith(normal):
                raise ValueError('independent normal log wire boundary changed')
            raw = raw[:-len(normal)] + logs(5)
        if hashlib.sha256(raw).digest() != hashes[node, sequence]:
            raise ValueError('stress source differs from independent burst workload')
    return mapping


def missing_management_slots(management, began_ns, seconds):
    actual = {(r['started_unix_ns']-began_ns)//(NS//2) for r in management}
    return sorted(set(range(seconds*2))-actual)


def first_creation(path, stop):
    deadline = time.monotonic()+30
    while not stop.is_set() and time.monotonic() < deadline:
        candidates = []
        if path.exists():
            with path.open() as source:
                for _ in range(10000):
                    line = source.readline(2*1024**2)
                    if not line or not line.endswith('\n'):
                        break
                    event = json.loads(line)
                    if event['e'] == 'created' and event['seq'] == 1:
                        candidates.append(event)
        if candidates:
            return min(candidates, key=lambda event: event['t'])
        stop.wait(.05)
    raise RuntimeError('original first creation unavailable before attack scheduling')


def reject_probe(fixture, token, body):
    request = urllib.request.Request(fixture.origin + '/v1/batches', data=body,
        headers={'authorization': 'Bearer ' + token, 'content-type': 'application/x-protobuf'})
    try:
        with urllib.request.urlopen(request, context=fixture.context, timeout=15) as response:
            response.read(65537)
            return response.status
    except urllib.error.HTTPError as error:
        error.read(65537)
        return error.code


def perform(f, args, summary):
    identities, seconds, burst, rounds, attack_at = ((10, 12, (4, 6), 10, 2) if args.smoke
                                                   else (100, 180, (60, 80), 200, 50))
    rng = random.Random(args.seed)
    enrollments = [f.enroll(f'sim{i:04d}') for i in range(identities)]
    probes = [f.enroll(f'probe{i}') for i in range(10)]
    for i in range(5):
        status, _ = f.bridge.request(f'/v1/console/nodes/probe{i}/revoke', 'POST')
        if status != 200:
            raise RuntimeError('probe credential revocation failed')
        time.sleep(.15)
    reader = f.reader([e['enrollment_id'] for e in enrollments])
    tokens = f.work / 'tokens'
    tokens.write_text(''.join(e['token'] + '\n' for e in enrollments)); tokens.chmod(0o600)
    f.bridge.poll_ui(True)
    last_ui_health = time.monotonic()
    sim = f.spawn([f.helpers / 'spindle_sim', '--server-url', f.origin, '--ca', f.work / 'ca.pem',
                   '--tokens', tokens, '--seed', hex(args.seed), '--seconds', str(seconds),
                   '--workers', str(identities), '--out', f.work / 'sim',
                   '--burst-from', str(burst[0]), '--burst-to', str(burst[1]), '--burst-factor', '5'], 'sim')
    began = time.monotonic()
    failures, management, samples, rejected, rejections = [], [], [], [], []
    attack_clock = {}
    stop = threading.Event()

    def manager():
        while not stop.is_set():
            started = time.monotonic()
            started_unix_ns = time.time_ns()
            try:
                status, _ = f.bridge.request('/v1/console/nodes')
                if status != 200:
                    raise RuntimeError('inventory refused')
                node = f'sim{rng.randrange(identities):04d}'
                status, _ = f.bridge.request(f'/v1/console/nodes/{node}/config', 'PUT',
                    {'logs': [], 'metric_interval_s': rng.choice([15, 30])})
                if status != 200:
                    raise RuntimeError('configuration refused')
                now = time.time_ns()
                elapsed, pages = reader.pages({'kind': 'logs', 'node': node,
                    'from_ns': now-30*NS, 'to_ns': now, 'limit': 1000})
                if any(not page.get('complete') for page in pages):
                    raise RuntimeError('incomplete concurrent query')
                management.append({'elapsed_s': time.monotonic()-started,
                                   'started_unix_ns': started_unix_ns,
                                   'query_s': elapsed, 'pages': len(pages)})
            except Exception as error:
                failures.append('management: ' + repr(error))
            stop.wait(max(0, .5-(time.monotonic()-started)))

    def attacker():
        try:
            original = first_creation(f.work / 'sim/events.jsonl', stop)
            before = time.monotonic_ns()
            wall = time.time_ns()
            after = time.monotonic_ns()
            # A seq1 observation is an upper bound for simulator begin, not
            # its exact epoch. The after-read mapping deliberately starts late.
            target = after + original['t'] + attack_at*NS - wall
            attack_clock.update(original_first_creation=original, wall_ns=wall,
                                monotonic_before_ns=before, monotonic_after_ns=after,
                                scheduled_monotonic_ns=target)
            stop.wait(max(0, (target-time.monotonic_ns())/NS))
        except Exception as error:
            failures.append('rejection scheduling: ' + repr(error))
            return
        for n in range(rounds):
            if stop.is_set():
                break
            node = hashlib.sha256(f'rejection:{args.seed}:{n}'.encode()).digest()[:16]
            rejected.append(node.hex())
            body = batch_bytes(node, 1)
            row = {'round': n, 'node_id': node.hex(), 'elapsed_s': time.monotonic()-began,
                   'started_unix_ns': time.time_ns(), 'attempts': []}
            rejections.append(row)
            for name, token, payload in [
                ('revoked', probes[n % 5]['token'], body),
                ('unknown', hashlib.sha256(f'unknown:{args.seed}:{n}'.encode()).hexdigest(), body),
                ('malformed', probes[5+n % 5]['token'], b'\xff'*(n+1))]:
                attempt = {'kind': name, 'started_unix_ns': time.time_ns(),
                           'body_sha256': hashlib.sha256(payload).hexdigest(), 'body_bytes': len(payload)}
                row['attempts'].append(attempt)
                try:
                    row[name] = reject_probe(f, token, payload)
                    attempt['status'] = row[name]
                except Exception as error:
                    row[name] = None
                    attempt['error'] = repr(error)
                    failures.append('rejection: ' + repr(error))
                finally:
                    attempt['finished_unix_ns'] = time.time_ns()
            stop.wait(.2)

    threads = [threading.Thread(target=fn, daemon=True) for fn in (manager, attacker)]
    for thread in threads:
        thread.start()
    try:
        while True:
            samples.append(f.resource_sample())
            if time.monotonic() - last_ui_health >= 25:
                f.bridge.poll_health()
                last_ui_health = time.monotonic()
            if sim.poll() is not None:
                finished = json.loads((f.work / 'sim/sim-summary.json').read_text())
                if time.time_ns() >= finished['began_unix_ns'] + seconds*NS:
                    break
            if time.monotonic()-began > seconds+120:
                raise RuntimeError('burst workload exceeded drain deadline')
            time.sleep(.5)
    finally:
        stop.set()
        for thread in threads:
            thread.join(timeout=30)
    if any(thread.is_alive() for thread in threads):
        raise RuntimeError('concurrent pressure worker did not terminate')
    ui = f.bridge.poll_health()
    f.bridge.poll_ui(False)
    samples.append(f.resource_sample())
    f.stop_server()
    spool_dir = f.work / 'companion-custody'
    observation = soak_companion.dump_stopped_spool(f.helpers / 'spool_dump',
        f.work / 'state/self-spindle/node.conf', spool_dir, processes_stopped=True)
    observed, companion_ledger = native_custody(records(spool_dir / 'spool.jsonl'), f.out / 'server-0.out')
    if observed != observation:
        raise RuntimeError('independent companion observations differ')
    recovered_file = f.work / 'recovered.jsonl'
    with recovered_file.open('wb') as out:
        subprocess.run([f.helpers / 'server_dump', f.config], stdout=out,
                       stderr=subprocess.PIPE, check=True, timeout=120)
    recovered = records(recovered_file)
    projected = [soak_companion.project(record) for record in recovered]
    sim_ledger = records(f.work / 'sim/transcript.jsonl')
    sim_events = records(f.work / 'sim/events.jsonl')
    verify_offers(args.seed, identities, seconds, burst, sim_events, sim_ledger)
    ledger = sim_ledger + companion_ledger + projected + [{'type': 'end'}]
    verdict = delivery_oracle.check(json.dumps(record) for record in ledger)
    custody = soak_companion.validate_recovery(observation, projected)
    # A successful baseline must reject a real recovered-row deletion through
    # the unchanged checker, rather than accepting an unrelated setup failure.
    acknowledged = {(r['node_id'], r['generation'], r['sequence']) for r in ledger
                    if r['type'] == 'response' and r['kind'] == 'ack'}
    recovered_index = next(i for i,r in enumerate(ledger) if r['type'] == 'recovered'
        and (r['node_id'], r['generation'], r['sequence']) in acknowledged)
    defect = delivery_oracle.check(json.dumps(r) for i,r in enumerate(ledger) if i != recovered_index)
    made, acked, starts = {}, {}, {}
    for event in sim_events:
        key = event['id'], event.get('seq')
        if event['e'] == 'created':
            made[key] = event['t']
        elif event['e'] == 'attempt':
            starts.setdefault(key, event['start'])
            if event['kind'] == 'ack':
                acked.setdefault(key, event['end'])
    sim_summary = json.loads((f.work / 'sim/sim-summary.json').read_text())
    started_ns = sim_summary['began_unix_ns']
    actual_attack_start = ((rejections[0]['started_unix_ns']-started_ns)/NS if rejections else None)
    attack_clock['authoritative_simulator_begin_ns'] = started_ns
    attack_clock['actual_first_round_s'] = actual_attack_start
    overlap = [r['round'] for r in rejections
               if burst[0]*NS <= r['started_unix_ns']-started_ns < burst[1]*NS]
    missing_slots = missing_management_slots(management, started_ns, seconds)
    at = [burst[0]-1 if args.smoke else 55, seconds-1 if args.smoke else 175]
    backlog = [sum(c <= started_ns+t*NS and acked.get(k, 1 << 127) > started_ns+t*NS
                   for k,c in made.items()) for t in at]
    latency = {}
    for name, left, right in [('before', 0, burst[0]), ('burst', *burst), ('after', burst[1], seconds)]:
        keys = [k for k,c in made.items() if left*NS <= c-started_ns < right*NS]
        values = [(acked[k]-made[k])/NS for k in keys if k in acked]
        sends = [(acked[k]-starts[k])/NS for k in keys if k in acked]
        latency[name] = {'created': len(keys), 'censored': sum(k not in acked for k in keys),
                         'creation_to_ack_p99_s': p99(values), 'creation_to_ack_max_s': max(values, default=None),
                         'first_send_to_ack_p99_s': p99(sends)}
    rejected_set = set(rejected)
    gates = {'delivery_oracle': verdict.passed, 'source_population': True,
             'drop_recovered_control': verdict.passed and not defect.passed
                 and any(v['rule'] == 'ACKED-DURABLE' for v in defect.violations),
             'companion_custody': custody['passed'] and not observation['unacked_sequences'],
             'sim_exit': sim.returncode == 0,
             'rejections': len(rejections) == rounds and all(r['revoked'] == r['unknown'] == 401
                           and r['malformed'] in (400, 413) for r in rejections),
             'rejection_timing': actual_attack_start is not None and actual_attack_start >= attack_at
                 and (args.smoke or (actual_attack_start < attack_at+1 and bool(overlap))),
             'no_rejected_commit': not any(r['node_id'] in rejected_set for r in recovered),
             'backlog': backlog[1] <= backlog[0]+identities and len(made) == len(acked),
             'management': not failures and not missing_slots,
             'ui': ui['completed'] >= (1 if args.smoke else 30)
                 and ui['statusFailures'] == ui['transportFailures'] == 0,
             'rss': max(s['server_hwm_kib'] for s in samples) <= 2*1024**2,
             'no_oom': all('oom_kill 0' in v['memory.events'] for s in samples for v in s['groups'].values())}
    summary.update(passed=all(gates.values()), gates=gates, identities=identities, seconds=seconds,
                   burst=burst, failures=failures, resources=samples, rejections=rejections,
                   attack_clock=attack_clock, attack_rounds_during_burst=overlap,
                   created=len(made), acked=len(acked), backlog=backlog, latency=latency, ui=ui,
                   management=management, missing_management_slots=missing_slots, companion_custody=custody,
                   violations=verdict.violations,
                   defect_violations=defect.violations)
    archive = f.archive_dir()
    for source in [recovered_file, f.work/'sim/events.jsonl', f.work/'sim/transcript.jsonl', spool_dir/'spool.jsonl']:
        with source.open('rb') as incoming, gzip.open(archive/(source.parent.name+'-'+source.name+'.gz'), 'wb') as output:
            shutil.copyfileobj(incoming, output)
    with gzip.open(archive/'custody-ledger.jsonl.gz', 'wt') as output:
        for record in ledger:
            output.write(json.dumps(record)+'\n')
    if sum(directory_bytes(p) for p in [f.work, f.out, archive]) + 512*1024**2 > 5*1024**3:
        raise RuntimeError('combined fixture/archive/browser allowance exceeds 5 GiB')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--deb-receipt', required=True)
    parser.add_argument('--rpm-receipt', required=True)
    parser.add_argument('--out', required=True)
    parser.add_argument('--seed', type=lambda v: int(v, 0), choices=SEEDS, required=True)
    parser.add_argument('--smoke', action='store_true')
    parser.add_argument('--server-cpus', default='0-1')
    parser.add_argument('--worker-cpus', default='2-3')
    args = parser.parse_args()
    summary = {'classification': 'disposable pressure smoke' if args.smoke else 'registered pressure cell',
               'seed': args.seed, 'passed': False}
    with CandidateFixture(args.deb_receipt, args.rpm_receipt, args.out,
                          server_cpus=args.server_cpus, supervisor_cpus=args.worker_cpus) as f:
        try:
            perform(f, args, summary)
            f.receipt['passed'] = summary['passed']
        except BaseException as error:
            summary.update(failure_type=type(error).__name__, failure=str(error)[:1200])
            raise
        finally:
            (f.out/'summary.json').write_text(json.dumps(summary, indent=2)+'\n')
    return 0 if summary['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
