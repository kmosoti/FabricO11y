#!/usr/bin/env python3
"""Finite native reference trials; run through the owning contained hammer job."""
import argparse
import csv
import hashlib
import importlib.metadata
import json
import os
from pathlib import Path
import resource
import shutil
import signal
import subprocess
import sys
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools/bench/labs/cross_system'))
import query_sweep as q
import vortex_sweep as v

WHEELS = (*q.WHEELS, *v.EXTRA_WHEELS)
SEEDS = (2704001, 2704002, 2704003)
N, BATCH = 500000, 8192
ARMS = ('arrow-parquet-scan', 'duckdb-read-parquet', 'vortex-predicate')
FIELDS = ('id', 'observed_ns', 'node_id', 'body')
QUERIES = (
    {'name': 'time-top50', 'low': 200000, 'high': 225000},
    {'name': 'node-time-top50', 'low': 100000, 'high': 400000, 'node': 'node-0042'},
    {'name': 'rare-text-top50', 'low': 0, 'high': N, 'text': 'RAREmarker'},
    {'name': 'absent-text-top50', 'low': 0, 'high': N, 'text': 'ABSENTmarker'},
    {'name': 'full-count-body-length', 'aggregate': True},
)


def admission(work, out):
    if q.footprint(work) >= 8 * 1024**3 or q.footprint(out) >= 512 * q.MIB:
        raise RuntimeError('8GiB scratch/512MiB evidence boundary reached')
    if shutil.disk_usage(work).free < 8 * 1024**3:
        raise RuntimeError('8GiB disk reserve unavailable')


def measured(operation):
    cpu, wall = time.process_time_ns(), time.perf_counter_ns()
    value = operation()
    return value, {'wall_ns': time.perf_counter_ns() - wall,
                   'process_cpu_ns': time.process_time_ns() - cpu,
                   'process_cumulative_hwm_kib': resource.getrusage(resource.RUSAGE_SELF).ru_maxrss}


def row(seed, i):
    # Every body is 512 ASCII bytes. Random halves use a full 256-byte SHAKE
    # output represented as hex, rather than a short repeated random motif.
    prefix = 'RAREmarker:' if i % 10000 == 0 else 'telemetry:'
    filler = 'steady-node-event;' * 32 if i % 2 == 0 else hashlib.shake_256(f'{seed}:{i}'.encode()).hexdigest(256)
    return (i, seed * 1000000000 + i, f'node-{i % 1000:04d}', prefix + filler[:512 - len(prefix)])


def matches(r, query, seed):
    return (query['low'] <= r[1] - seed * 1000000000 < query['high']
            and ('node' not in query or r[2] == query['node'])
            and ('text' not in query or query['text'] in r[3]))


def verify(actual, expected):
    if actual != expected:
        raise RuntimeError('complete reference output differs: value/order/missing/extra')


def controls():
    expected = {'rows': [[1, 'first'], [2, 'second']], 'count': 2}
    bads = {'missing': {'rows': [[1, 'first']], 'count': 2},
            'extra': {'rows': [[1, 'first'], [2, 'second'], [3, 'extra']], 'count': 3},
            'changed': {'rows': [[1, 'changed'], [2, 'second']], 'count': 2},
            'ordered': {'rows': [[2, 'second'], [1, 'first']], 'count': 2},
            'count': {'rows': expected['rows'], 'count': 3}}
    rejected = []
    for name, bad in bads.items():
        try:
            verify(bad, expected)
        except RuntimeError:
            rejected.append(name)
        else:
            raise RuntimeError('checker accepted injected defect: ' + name)
    verify(expected, expected)
    return {'origin': 'fixed independent two-row complete-output fixture', 'rejected': rejected}


def worker(args):
    import duckdb
    import pyarrow as pa
    import pyarrow.compute as pc
    import pyarrow.parquet as pq
    import vortex as vx
    import vortex.expr as ve
    pa.set_cpu_count(2)
    pa.set_io_thread_count(2)
    trial, out, seed = args.trial, args.out, args.seed
    checker_controls = controls()
    q.dump(out / 'checker-controls.json', checker_controls)
    schema = pa.schema([pa.field(n, t, nullable=False) for n, t in zip(FIELDS,
                       (pa.uint64(), pa.uint64(), pa.string(), pa.string()))])
    expected = {item['name']: {'rows': [], 'count': 0} for item in QUERIES[:-1]}
    expected[QUERIES[-1]['name']] = {'count': N, 'body_length_sum': N * 512}
    source_digest = hashlib.sha256()
    def construct():
        batches = []
        for first in range(0, N, BATCH):
            rows = [row(seed, i) for i in range(first, min(first + BATCH, N))]
            for r in rows:
                source_digest.update((json.dumps(r, separators=(',', ':')) + '\n').encode())
                for query in QUERIES[:-1]:
                    if matches(r, query, seed):
                        answer = expected[query['name']]
                        answer['count'] += 1
                        if len(answer['rows']) < 50:
                            answer['rows'].append([r[0], r[3]])
            batches.append(pa.RecordBatch.from_arrays(
                [pa.array([r[j] for r in rows], type=schema.types[j]) for j in range(4)], schema=schema))
        return pa.Table.from_batches(batches, schema=schema)
    table, generation = measured(construct)
    q.dump(out / 'expected.json', expected)
    q.dump(out / 'source-dataset.json', {'seed': seed, 'rows': N, 'body_bytes': 512,
        'schema': str(schema), 'canonical_jsonl_sha256': source_digest.hexdigest(),
        'canonicalization': 'ASCII JSON arrays, compact separators, newline, ascending id',
        'generation_and_arrow_conversion': generation})
    parquet, vortex = trial / 'logs.parquet', trial / 'logs.vortex'
    _, pw = measured(lambda: pq.write_table(table, parquet, row_group_size=BATCH,
                      compression='zstd', compression_level=3, write_statistics=True))
    _, vw = measured(lambda: vx.io.write(table, str(vortex)))
    del table
    connection = duckdb.connect()
    connection.execute('SET threads=2')
    connection.execute("SET memory_limit='2GB'")
    connection.execute("SET temp_directory='" + str(trial / 'duck-temp').replace("'", "''") + "'")
    connection.execute('SET autoinstall_known_extensions=false')
    connection.execute('SET autoload_known_extensions=false')
    vf = vx.open(str(vortex))

    def exact(decoded, query):
        if query.get('aggregate'):
            return {'count': decoded.num_rows, 'body_length_sum': pc.sum(pc.utf8_length(decoded['body'])).as_py()}
        low, high = (seed * 1000000000 + query[k] for k in ('low', 'high'))
        mask = pc.and_(pc.greater_equal(decoded['observed_ns'], low), pc.less(decoded['observed_ns'], high))
        if 'node' in query:
            mask = pc.and_(mask, pc.equal(decoded['node_id'], query['node']))
        if 'text' in query:
            mask = pc.and_(mask, pc.match_substring(decoded['body'], query['text']))
        selected = decoded.filter(mask)
        rows = selected.select(['id', 'body']).sort_by([('id', 'ascending')]).slice(0, 50).to_pylist()
        return {'rows': [[r['id'], r['body']] for r in rows], 'count': selected.num_rows}

    def execute(arm, query):
        if arm == 'duckdb-read-parquet':
            if query.get('aggregate'):
                values = connection.execute('SELECT count(*), sum(length(body)) FROM read_parquet(?)', [str(parquet)]).fetchone()
                return {'count': values[0], 'body_length_sum': values[1]}
            params = [str(parquet), seed * 1000000000 + query['low'], seed * 1000000000 + query['high']]
            where = 'observed_ns >= ? AND observed_ns < ?'
            if 'node' in query:
                where += ' AND node_id = ?'
                params.append(query['node'])
            if 'text' in query:
                where += ' AND contains(body, ?)'
                params.append(query['text'])
            # count window is consumed with every projected result; empty answer
            # count is zero. ORDER BY precedes LIMIT in SQL as in the oracle.
            records = connection.execute('SELECT id, body, count(*) OVER () FROM read_parquet(?) WHERE '
                                         + where + ' ORDER BY id LIMIT 50', params).fetchall()
            return {'rows': [[r[0], r[1]] for r in records], 'count': records[0][2] if records else 0}
        if arm == 'arrow-parquet-scan':
            decoded = pq.read_table(parquet, use_threads=True)
        else:
            expr = None
            if not query.get('aggregate'):
                expr = ((ve.column('observed_ns') >= ve.literal(vx.uint(64), seed * 1000000000 + query['low']))
                        & (ve.column('observed_ns') < ve.literal(vx.uint(64), seed * 1000000000 + query['high'])))
                if 'node' in query:
                    expr = expr & (ve.column('node_id') == query['node'])
                if 'text' in query:
                    expr = expr & ve.like(ve.column('body'), '%' + query['text'] + '%')
            array = vf.scan(expr=expr).read_all().to_arrow_array()
            decoded = (pa.Table.from_batches([], schema=pa.schema(list(array.type))) if len(array) == 0
                       else pa.Table.from_struct_array(array)).cast(schema)
        return exact(decoded, query)

    measurements = []
    for qi, query in enumerate(QUERIES):
        for repetition in range(3):
            shift = (qi + repetition + SEEDS.index(seed)) % len(ARMS)
            order = ARMS[shift:] + ARMS[:shift]
            for arm in order:
                answer, metrics = measured(lambda: execute(arm, query))
                verify(answer, expected[query['name']])
                measurements.append({'seed': seed, 'query': query['name'], 'arm': arm,
                    'repetition': repetition, 'order': list(order), **metrics,
                    'materialization': 'native projection+SQL window count' if arm.startswith('duckdb')
                                       else 'full scan Arrow materialization' if arm.startswith('arrow')
                                       else 'predicate-selected full rows to Arrow materialization',
                    'exact_output': answer})
                q.dump(out / 'measurements.json', measurements)
    connection.close()
    with (out / 'measurements.csv').open('w', newline='') as stream:
        columns = ['seed', 'query', 'arm', 'repetition', 'wall_ns', 'process_cpu_ns', 'process_cumulative_hwm_kib', 'materialization']
        writer = csv.DictWriter(stream, fieldnames=columns, extrasaction='ignore')
        writer.writeheader()
        writer.writerows(measurements)
    q.dump(out / 'result.json', {'seed': seed, 'versions': {name: importlib.metadata.version(name) for name, *_ in WHEELS},
        'construction': {'generation_and_arrow_conversion': generation, 'parquet_write': pw, 'vortex_write': vw},
        'files': {p.name: {'bytes': p.stat().st_size, 'sha256': q.sha(p)} for p in (parquet, vortex)},
        'measurements': len(measurements), 'checker_controls': checker_controls,
        'limitations': ['Native wheel versions differ from reviewed upstream source pins.',
            'Two CPU affinity slots and two configured engine threads; no cache flush.',
            'Arrow scans full columns, Vortex predicates select rows, DuckDB projects and aggregates natively.',
            'Vortex text LIKE uses needles without wildcard characters; exact Arrow literal recheck follows.',
            'Top50 results ordered by id include full body and complete matching count.',
            'RSS HWM is cumulative process scope, never an operation allocation or memory peak.',
            'Finite synthetic telemetry trial; no production migration or capacity conclusion.']})


def child(argv, env, out, deadline, work, evidence):
    receipt = {'argv': argv, 'exit': None, 'rss_samples': []}
    with out.with_suffix('.out').open('wb') as so, out.with_suffix('.err').open('wb') as se:
        process = subprocess.Popen(argv, cwd=ROOT, env=env, stdout=so, stderr=se, start_new_session=True)
        start = time.monotonic()
        try:
            while process.poll() is None:
                admission(work, evidence)
                if time.monotonic() >= deadline:
                    raise RuntimeError('reference deadline exhausted')
                try:
                    status = Path(f'/proc/{process.pid}/status').read_text()
                    rss = next(int(line.split()[1]) for line in status.splitlines() if line.startswith('VmRSS:'))
                    receipt['rss_samples'].append({'elapsed_seconds': time.monotonic() - start, 'rss_kib': rss})
                except (FileNotFoundError, ProcessLookupError, StopIteration):
                    pass
                time.sleep(.2)
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=10)
            receipt['exit'] = process.returncode
            q.dump(out.with_suffix('.command.json'), receipt)
    if process.returncode:
        raise RuntimeError(f'reference child exit {process.returncode}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--seconds', type=int, default=1000)
    parser.add_argument('--worker', action='store_true')
    parser.add_argument('--trial', type=Path)
    parser.add_argument('--seed', type=int, choices=SEEDS)
    args = parser.parse_args()
    q.require_limits()
    if args.worker:
        worker(args)
        return
    if not 1 <= args.seconds <= 1000:
        parser.error('--seconds must be between 1 and 1000')
    if sys.version_info[:2] != (3, 14) or os.uname().machine != 'x86_64':
        raise RuntimeError('pinned cp314 x86_64 platform required')
    affinity = sorted(os.sched_getaffinity(0))
    if len(affinity) < 2:
        raise RuntimeError('two CPU affinity slots required')
    os.sched_setaffinity(0, affinity[:2])
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    storage = Path('/run/media/kmosoti/data/FabricO11y/scratch')
    if not scratch.is_relative_to(storage) or not Path(os.environ['TMPDIR']).resolve().is_relative_to(storage):
        raise RuntimeError('launcher-owned data-drive scratch required')
    out = args.out.resolve()
    if out.exists() or not out.is_relative_to(ROOT / 'docs/experiments/benchmarks/data'):
        raise RuntimeError('fresh repository evidence output required')
    work = scratch / ('hammer-references-' + out.name)
    work.mkdir()
    out.mkdir(parents=True)
    wheels = work / 'wheels'
    wheels.mkdir()
    deadline, status = time.monotonic() + args.seconds, 'interrupted'
    try:
        admission(work, out)
        files = [Path(__file__).resolve(), Path(q.__file__).resolve(), Path(v.__file__).resolve(), ROOT / 'tools/resource_group.py']
        q.dump(out / 'source-manifest.json', {str(p.relative_to(ROOT)): q.sha(p) for p in files})
        for p in files:
            shutil.copy2(p, out / p.name)
        q.dump(out / 'environment.json', {'argv': sys.argv, 'python': sys.version, 'uname': list(os.uname()),
            'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
            'wheel_pins': WHEELS, 'affinity': sorted(os.sched_getaffinity(0)), 'scratch': str(work),
            'seconds': args.seconds, 'scratch_limit_bytes': 8 * 1024**3, 'evidence_limit_bytes': 512 * q.MIB})
        for name, version, filename, base, digest, size in WHEELS:
            path = wheels / filename
            with urllib.request.urlopen(base + filename, timeout=25) as response, path.open('wb') as stream:
                total = 0
                while chunk := response.read(q.MIB):
                    total += len(chunk)
                    if total > size or time.monotonic() >= deadline:
                        raise RuntimeError('wheel download size/deadline exceeded')
                    stream.write(chunk)
                    admission(work, out)
            if total != size or q.sha(path) != digest:
                raise RuntimeError('wheel hash/size mismatch')
        requirements = work / 'requirements.txt'
        requirements.write_text(''.join(f'{wheels / filename} --hash=sha256:{digest}\n' for _, _, filename, _, digest, _ in WHEELS))
        env = dict(os.environ)
        env.pop('UV_CACHE_DIR', None)
        child(['uv', '--no-cache', 'pip', 'install', '--link-mode', 'copy', '--python', sys.executable,
               '--target', str(work / 'packages'), '--no-deps', '--no-index', '--require-hashes', '-r', str(requirements)],
              env, out / 'install', deadline, work, out)
        env.update(PYTHONPATH=str(work / 'packages'), PYTHONNOUSERSITE='1', OMP_NUM_THREADS='2', RAYON_NUM_THREADS='2', TOKIO_WORKER_THREADS='2')
        results = []
        for seed in SEEDS:
            trial, evidence = work / str(seed), out / str(seed)
            trial.mkdir()
            evidence.mkdir()
            child([sys.executable, str(Path(__file__).resolve()), '--worker', '--seed', str(seed),
                   '--trial', str(trial), '--out', str(evidence)], env, evidence / 'worker', deadline, work, out)
            result = json.loads((evidence / 'result.json').read_text())
            if result['measurements'] != 45 or result['versions'] != {n: ver for n, ver, *_ in WHEELS}:
                raise RuntimeError('reference trial coverage/version mismatch')
            results.append(result)
            q.dump(out / 'results.json', results)
            admission(work, out)
            shutil.rmtree(trial)
        status = 'passed'
    except BaseException as error:
        status = 'interrupted' if isinstance(error, KeyboardInterrupt) else 'failed'
        q.dump(out / 'failure.json', {'status': status, 'error': repr(error), 'preserved_scratch': str(work)})
        raise
    finally:
        occupied = q.footprint(work)
        if status == 'passed':
            shutil.rmtree(work)
        q.dump(out / 'cleanup.json', {'status': status, 'scratch': str(work), 'bytes_before_cleanup': occupied,
            'removed': not work.exists(), 'evidence_bytes': q.footprint(out)})


if __name__ == '__main__':
    main()
