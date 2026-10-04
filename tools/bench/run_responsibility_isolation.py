#!/usr/bin/env python3
"""Revision-2 native boundaries: preflight, calibrated sequential screen, owned cleanup."""
import argparse
import base64
import copy
import gzip
import hashlib
import json
import math
import os
import shutil
import signal
import statistics
import ssl
import subprocess
import sys
import time
import urllib.request
from collections import Counter, defaultdict
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / 'tools/qualification'))
import query_oracle
from delivery_faults import make_certs, free_port
sys.path.insert(0, str(REPO / 'tools/bench'))
from run_dev_small import process_stats


def dump(path, obj):
    path.write_text(json.dumps(obj, indent=2) + '\n')


def sha(path):
    h = hashlib.sha256()
    with path.open('rb') as f:
        for block in iter(lambda: f.read(2**20), b''):
            h.update(block)
    return h.hexdigest()


def footprint(root):
    total = 0
    for p in root.rglob('*'):
        try:
            if p.is_file():
                total += p.stat().st_size
        except FileNotFoundError:
            pass
    return total


def exact(expected, actual):
    if len(actual) != len(expected) or Counter(actual) != Counter(expected):
        raise AssertionError('missing/duplicate/changed bodies')


def custody(expected, actual):
    if sorted(expected) != sorted(actual):
        raise AssertionError('ACK sequence/hash custody mismatch')


def decode_records(path):
    records, bodies = [], []
    with path.open() as f:
        for line in f:
            r = json.loads(line)
            raw = bytes.fromhex(r['hex'])
            assert hashlib.sha256(raw).hexdigest() == r['sha256']
            batch = query_oracle.decode_batch(raw)
            assert not batch['gaps']
            records.append({'label': r['label'], 'received_ns': r['received_ns'],
                            'bytes': base64.b64encode(raw).decode()})
            bodies.extend(x['body'] for x in query_oracle.decode_logs_request(batch['logs_bytes']))
    return records, bodies


def quantiles(values):
    a = sorted(values)
    return {'samples': len(a), 'p50': a[math.ceil(len(a) * .5) - 1],
            'p99': a[math.ceil(len(a) * .99) - 1] if len(a) >= 1000 else None,
            'p99_status': 'exploratory' if len(a) >= 1000 else 'insufficient_population',
            'min': a[0], 'max': a[-1], 'sum': sum(a)}


def cgroup():
    out = {'mono_ns': time.monotonic_ns()}
    for name in ['cpu.max', 'cpu.stat', 'cpu.pressure', 'memory.max', 'memory.current',
                 'memory.stat', 'memory.events', 'memory.pressure', 'io.pressure']:
        p = Path('/sys/fs/cgroup') / name
        out[name] = p.read_text().strip() if p.exists() else None
    return out


def samples(pid):
    return {'mono_ns': time.monotonic_ns(), 'process': process_stats(pid), 'cgroup': cgroup()}


def cases():
    out = []
    for mode in ['collection', 'delivery', 'storage', 'processing', 'query',
                 'control', 'scheduling', 'observation']:
        sizes = [1, 20, 200] if mode == 'control' else [1, 2, 4] if mode == 'scheduling' else [900] if mode == 'observation' else [128, 900, 3500]
        for size in sizes:
            out.append({'id': f'{mode}-{size}', 'mode': mode, 'size': size,
                        'records': 4096, 'order': 'sorted'})
        if mode == 'processing':
            for order in ['reversed', 'shuffled']:
                out.append({'id': f'processing-900-{order}', 'mode': mode, 'size': 900,
                            'records': 4096, 'order': order})
            for n in [16384, 65536]:
                out.append({'id': f'processing-1024-{n}', 'mode': mode, 'size': 1024,
                            'records': n, 'order': 'shuffled'})
        if mode == 'scheduling':
            for workers in [1, 2, 4]:
                out.append({'id': f'scheduling-{workers}-65536', 'mode': mode, 'size': workers,
                            'records': 65536, 'body_size': 1024, 'order': 'shuffled'})
    return out


def negative_controls():
    for altered in [['original', 'changed'], ['original'], ['original', 'second', 'second']]:
        try:
            exact(['original', 'second'], altered)
        except AssertionError:
            pass
        else:
            raise AssertionError('body negative control accepted')
    try:
        custody([(1, 'abc')], [(1, 'changed')])
    except AssertionError:
        pass
    else:
        raise AssertionError('ACK negative control accepted')
    return {'changed_body_rejected': True, 'missing_body_rejected': True,
            'duplicate_body_rejected': True, 'changed_ack_hash_rejected': True}


def compress(src, dest):
    with src.open('rb') as f, gzip.open(dest, 'wb', compresslevel=6) as g:
        shutil.copyfileobj(f, g, 2**20)


def grade_query(work, dest, expected, retain, control_required):
    verdicts, controls = [], {}
    layouts = {}
    paths = sorted(list(work.glob('answer-*.json')) + list(work.glob('answer-*.jsonl')))
    assert paths, 'no query answers retained'
    for path in paths:
        kind = path.name.split('-')[1]
        if kind not in layouts:
            records, bodies = decode_records(work / (kind + '-records') / 'records.jsonl')
            exact(expected, bodies)
            layouts[kind] = records
        records = layouts[kind]
        with path.open() as f:
            objects = [json.loads(f.read())] if path.suffix == '.json' else (json.loads(line) for line in f)
            for index, obj in enumerate(objects):
                verdict = query_oracle.check(records, obj['query'], [obj['answer']])
                verdicts.append({'file': path.name, 'index': index, 'verdict': verdict,
                                 'answer_sha256': hashlib.sha256(json.dumps(obj['answer'], sort_keys=True).encode()).hexdigest()})
                assert verdict['passed'], verdict
                if control_required and obj['answer']['rows'] and not controls:
                    for name in ['changed', 'missing', 'duplicate']:
                        bad = copy.deepcopy(obj['answer'])
                        if name == 'changed':
                            bad['rows'][0]['body'] += 'changed'
                        elif name == 'missing':
                            bad['rows'] = bad['rows'][1:]
                        else:
                            bad['rows'].append(copy.deepcopy(bad['rows'][0]))
                        assert not query_oracle.check(records, obj['query'], [bad])['passed'], name
                        controls[name + '_query_rejected'] = True
        if retain:
            compress(path, dest / (path.name + '.gz'))
    if control_required:
        assert len(controls) == 3
    return verdicts, controls


def run(root, bins, case, pair, variant, cpus, phase, observer='minimal'):
    mode, size = case['mode'], case['size']
    label = f"{case['id']}-p{pair}-{variant}-{observer}"
    work, dest = root / 'scratch' / label, root / 'evidence' / label
    work.mkdir()
    (work / 'owned').write_text(label)
    dest.mkdir()
    free_before = shutil.disk_usage(work).free
    assert free_before > 4 * 2**30
    body_size = case.get('body_size', 900 if mode in ['control', 'scheduling'] else size)
    prediction = case['records'] * max(body_size, 128) * (25 if mode == 'query' else 10) + 100 * 2**20
    assert prediction <= 2**30, 'predicted trial scratch beyond bound'
    command = ['prlimit', f'--as={2 * 2**30}', '--', 'taskset', '-c', ','.join(map(str, cpus)),
               str(bins / variant), str(work), mode, str(size)]
    bench_env = {'BENCH_RECORDS': str(case['records']), 'BENCH_ORDER': case['order'],
                 'BENCH_BODY_SIZE': str(body_size), 'BENCH_QUERY_REPEATS': '3',
                 'BENCH_OBSERVER': 'detailed' if observer == 'detailed' else 'minimal',
                 'BENCH_PREFLIGHT': '1' if phase == 'preflight' else '0',
                 'BENCH_QUERY_ROTATION': str(max(pair - 1, 0) % 4)}
    env = dict(os.environ, **bench_env)
    server, p = None, None
    server_stats, mon = [], []
    baseline = cgroup()
    dump(dest / 'cgroup-before.json', baseline)
    retain = phase == 'measure' and pair == 1 and variant == 'plain'
    try:
        if mode == 'delivery':
            make_certs(work)
            port, admin = free_port(), os.urandom(32).hex()
            (work / 'admin').write_text(admin)
            conf = work / 'server.conf'
            conf.write_text(f'listen=127.0.0.1:{port}\ntls_cert={work}/server.pem\ntls_key={work}/server.key\nstate_dir={work}/state\nadmin_token_file={work}/admin\njournal_bytes=268435456\njournal_file_bytes=67108864\nseal_workers=1\n')
            server_command = ['prlimit', f'--as={2 * 2**30}', '--', 'taskset', '-c', ','.join(map(str, cpus)),
                              str(bins / 'fabric-server'), 'serve', str(conf)]
            dump(dest / 'server-command.json', server_command)
            with (work / 'server.out').open('wb') as sout, (dest / 'server.err').open('wb') as serr:
                server = subprocess.Popen(server_command, stdout=sout, stderr=serr)
            ctx = ssl.create_default_context(cafile=str(work / 'ca.pem'))
            url = f'https://127.0.0.1:{port}'
            for _ in range(100):
                try:
                    req = urllib.request.Request(url + '/v1/admin/nodes', data=json.dumps({'name': 'fixture', 'metric_interval_s': 15, 'logs': []}).encode(), headers={'authorization': 'Bearer ' + admin, 'content-type': 'application/json'}, method='POST')
                    with urllib.request.urlopen(req, context=ctx, timeout=1) as r:
                        token = json.loads(r.read())['token']
                    break
                except urllib.error.URLError:
                    if server.poll() is not None:
                        raise RuntimeError('server startup failed')
                    time.sleep(.05)
            else:
                raise RuntimeError('server startup timeout')
            (work / 'token').write_text(token)
            command.extend([url, str(work / 'ca.pem'), str(work / 'token')])
            server_stats.append(samples(server.pid))
        dump(dest / 'command.json', {'argv': command, 'bench_env': bench_env, 'case': case,
                                     'phase': phase, 'observer': observer, 'predicted_logical_bytes': prediction})
        start = time.monotonic()
        next_sample = start
        next_space = start
        interval = .1 if observer == 'detailed' else .5
        with (work / 'timings.jsonl').open('wb') as out, (dest / 'stderr.txt').open('wb') as err:
            p = subprocess.Popen(command, stdout=out, stderr=err, env=env)
            while p.poll() is None:
                now = time.monotonic()
                if now - start > 180:
                    raise RuntimeError('child wall bound')
                if now >= next_space:
                    if shutil.disk_usage(work).free < 4 * 2**30:
                        raise RuntimeError('free disk floor')
                    next_space = now + .5
                if observer != 'minimal' and now >= next_sample:
                    try:
                        mon.append(samples(p.pid))
                    except (FileNotFoundError, ProcessLookupError, KeyError):
                        pass
                    if server is not None:
                        server_stats.append(samples(server.pid))
                    next_sample = now + interval
                time.sleep(.01)
        assert p.returncode == 0, f'probe exit {p.returncode}'
        elapsed = time.monotonic() - start
        dump(dest / 'cgroup-after-timing.json', cgroup())
        if server is not None:
            server_stats.append(samples(server.pid))
            server.send_signal(signal.SIGTERM)
            server.wait(timeout=30)
            with (work / 'replay.jsonl').open('wb') as f, (dest / 'dump.err').open('wb') as err:
                subprocess.run([str(bins / 'server_dump'), str(work / 'server.conf'), '--records'], stdout=f, stderr=err, check=True, timeout=60)
        assert footprint(work) <= 2**30, 'post-timing scratch bound'
        rows = [json.loads(line) for line in (work / 'timings.jsonl').read_text().splitlines()]
        assert rows[-1]['stage'] == 'complete'
        meta = rows[0]
        source = (work / 'source.log').read_bytes()
        assert hashlib.sha256(source).hexdigest() == meta['source_sha256']
        expected = source.decode().splitlines()
        records, bodies = decode_records(work / 'records.jsonl')
        exact(expected, bodies)
        oracle, controls = [], {}
        if mode == 'delivery':
            recovered, decoded = [], []
            with (work / 'replay.jsonl').open() as f:
                for line in f:
                    r = json.loads(line)
                    raw = base64.b64decode(r['bytes'])
                    batch = query_oracle.decode_batch(raw)
                    recovered.append((batch['sequence'], hashlib.sha256(raw).hexdigest()))
                    decoded.extend(x['body'] for x in query_oracle.decode_logs_request(batch['logs_bytes']))
            observed = [(r['sequence'], r['sha256']) for r in rows if r['stage'] == 'acked_hash']
            custody(observed, recovered)
            exact(expected, decoded)
            controls['exact_ack_custody'] = True
        if mode == 'query':
            oracle, controls = grade_query(work, dest, expected, retain, phase == 'preflight')
        stages = defaultdict(list)
        for row in rows:
            if 'wall_ns' in row:
                stages[row['stage']].append(row)
        stats = {}
        for name, data in stages.items():
            stats[name] = {'wall_ms': quantiles([r['wall_ns'] / 1e6 for r in data]),
                           'units': sum(r['units'] for r in data),
                           'cpu_ns': sum(r['cpu_ns'] for r in data),
                           'proc_io': {k: sum(r['proc_io_delta'][k] for r in data) for k in data[0]['proc_io_delta']},
                           'max_baseline_live_bytes': max((r['allocation']['baseline_live_bytes'] for r in data if r['allocation']), default=None),
                           'max_peak_live_bytes': max((r['allocation']['peak_live_bytes'] for r in data if r['allocation']), default=None),
                           'max_incremental_peak_bytes': max((r['allocation']['incremental_peak_bytes'] for r in data if r['allocation']), default=None),
                           'cumulative_requested_bytes': sum((r['allocation']['cumulative_requested_bytes'] for r in data if r['allocation']), 0)}
        summary = {'label': label, 'case_id': case['id'], 'mode': mode, 'size': size, 'pair': pair,
                   'variant': variant, 'observer': observer, 'phase': phase, 'elapsed_process_s': elapsed,
                   'exit': p.returncode, 'fixture': meta, 'complete': rows[-1],
                   'source_sha256': meta['source_sha256'], 'source_bytes': len(source),
                   'records_sha256': sha(work / 'records.jsonl'), 'records': len(expected),
                   'exact_fixture_bodies': True, 'oracle': oracle, 'controls': controls, 'stats': stats,
                   'sampled_peak_process_hwm_mib': max((s['process']['hwm_kib'] / 1024 for s in mon), default=None),
                   'sample_count': len(mon)}
        dump(dest / 'summary.json', summary)
        dump(dest / 'resources.json', mon)
        dump(dest / 'server-resources.json', server_stats)
        compress(work / 'timings.jsonl', dest / 'timings.jsonl.gz')
        if retain and not (mode == 'scheduling' and case['records'] == 65536):
            compress(work / 'records.jsonl', dest / 'records.jsonl.gz')
        return summary
    except Exception as error:
        dump(dest / 'failure.json', {'error': str(error), 'case': case, 'phase': phase})
        for name in ['timings.jsonl', 'records.jsonl', 'replay.jsonl']:
            if (work / name).exists():
                compress(work / name, dest / ('failure-' + name + '.gz'))
        for path in sorted(work.glob('answer-*')):
            compress(path, dest / ('failure-' + path.name + '.gz'))
        raise
    finally:
        if p is not None and p.poll() is None:
            p.kill()
            p.wait(timeout=10)
        if server is not None and server.poll() is None:
            server.kill()
            server.wait(timeout=10)
        removed = footprint(work)
        assert (work / 'owned').read_text() == label
        shutil.rmtree(work)
        dump(dest / 'cleanup.json', {'owned_directory': str(work), 'removed_logical_bytes': removed,
                                     'free_before': free_before, 'free_after': shutil.disk_usage(root).free,
                                     'removed': not work.exists()})
        dump(dest / 'cgroup-after-cleanup.json', cgroup())


def calibration_report(results):
    selected = {(r['pair'], r['variant'], r['observer']): r for r in results}
    stage = 'observer_hash_format_batch'
    baseline = [selected[(i, 'plain', 'minimal')]['stats'][stage]['wall_ms']['sum'] for i in range(1, 6)]
    comparisons = {}
    for label, variant, observer in [('allocator', 'counted', 'minimal'), ('lowrate_sampler', 'plain', 'lowrate'), ('detailed_100ms', 'plain', 'detailed')]:
        ratios = [selected[(i, variant, observer)]['stats'][stage]['wall_ms']['sum'] / baseline[i-1] for i in range(1, 6)]
        median = statistics.median(ratios)
        consistent = sum(x > 1 for x in ratios) >= 4 if median > 1 else sum(x < 1 for x in ratios) >= 4
        comparisons[label] = {'ratios': ratios, 'median_ratio': median,
                              'screening_signal': abs(median - 1) >= .1 and consistent,
                              'interpretation': 'screening only; preserved fresh-process variation'}
    empty = [r['stats']['observer_empty_span']['wall_ms']['p50'] for r in results if r['observer'] == 'minimal']
    median_empty_ms = statistics.median(empty)
    return {'stage': stage, 'baseline_wall_ms': baseline, 'comparisons': comparisons,
            'baseline_cv': statistics.stdev(baseline) / statistics.mean(baseline),
            'median_empty_boundary_ms': median_empty_ms,
            'recommended_minimum_boundary_ms_at_1percent': 100 * median_empty_ms,
            'interpretation': 'Use uncounted minimal main timing. Counted heap and detailed IO describe their instrumented variants; do not subtract observer cost.'}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--out', type=Path, required=True)
    ap.add_argument('--bins', type=Path, required=True)
    ap.add_argument('--phase', choices=['preflight', 'calibration', 'measure'], required=True)
    ap.add_argument('--readiness', type=Path)
    ap.add_argument('--calibration', type=Path)
    a = ap.parse_args()
    if a.phase != 'preflight':
        assert a.readiness and json.loads((a.readiness / 'complete.json').read_text())['phase'] == 'preflight'
    if a.phase == 'measure':
        assert a.calibration and json.loads((a.calibration / 'complete.json').read_text())['phase'] == 'calibration'
        assert (a.calibration / 'calibration-report.json').exists(), 'calibration interpretation not frozen'
    root, bins = a.out.resolve(), a.bins.resolve()
    assert not root.exists()
    root.mkdir()
    (root / 'scratch').mkdir()
    (root / 'evidence').mkdir()
    cpus = sorted(os.sched_getaffinity(0))
    dump(root / 'controls.json', negative_controls())
    environment = {'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=REPO, text=True).strip(),
                   'affinity': cpus, 'cgroup': cgroup(), 'binaries': {p.name: sha(p) for p in bins.iterdir() if p.is_file()},
                   'harness_sha256': sha(Path(__file__)), 'uname': list(os.uname()), 'phase': a.phase,
                   'matrix': cases(), 'readiness': str(a.readiness), 'calibration': str(a.calibration)}
    dump(root / 'environment.json', environment)
    for previous in [a.readiness, a.calibration]:
        if previous:
            env_before = json.loads((previous / 'environment.json').read_text())
            assert env_before['binaries'] == environment['binaries'], 'binary readiness mismatch'
            assert env_before['harness_sha256'] == environment['harness_sha256'], 'harness readiness mismatch'
    trials = []
    if a.phase == 'calibration':
        c = next(x for x in cases() if x['mode'] == 'observation')
        for pair in range(1, 6):
            order = ['minimal', 'lowrate', 'detailed'] if pair % 2 else ['detailed', 'lowrate', 'minimal']
            for observer in order:
                trials.append((c, pair, 'plain', observer))
            trials.append((c, pair, 'counted', 'minimal'))
    else:
        for c in cases():
            for pair in ([0] if a.phase == 'preflight' else range(1, 6)):
                for variant in (['plain', 'counted'] if pair % 2 else ['counted', 'plain']):
                    trials.append((c, pair, variant, 'minimal'))
    all_results, began = [], time.monotonic()
    try:
        for c, pair, variant, observer in trials:
            assert time.monotonic() - began < 3600, 'campaign wall bound'
            result = run(root, bins, c, pair, variant, cpus, a.phase, observer)
            all_results.append(result)
            print(json.dumps({'trial': result['label'], 'exit': result['exit'],
                              'elapsed_s': result['elapsed_process_s'], 'cleanup': True}), flush=True)
            assert footprint(root / 'evidence') <= 256 * 2**20, 'evidence budget'
        dump(root / 'summary.json', all_results)
        if a.phase == 'calibration':
            dump(root / 'calibration-report.json', calibration_report(all_results))
        dump(root / 'complete.json', {'phase': a.phase, 'trials': len(all_results),
                                      'seconds': time.monotonic() - began,
                                      'scratch_empty': not any((root / 'scratch').iterdir())})
    except Exception as error:
        dump(root / 'partial-summary.json', all_results)
        dump(root / 'status.json', {'phase': a.phase, 'status': 'failed', 'error': str(error),
                                  'completed_trials': len(all_results),
                                  'scratch_empty': not any((root / 'scratch').iterdir())})
        raise


if __name__ == '__main__':
    main()
