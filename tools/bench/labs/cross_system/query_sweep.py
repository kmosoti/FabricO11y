#!/usr/bin/env python3
"""Registered external query sweep; execute only inside resource_group.py."""
import argparse
import gzip
import hashlib
import json
import math
import os
from pathlib import Path
import random
import resource
import shutil
import signal
import statistics
import subprocess
import sys
import tarfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits

MIB = 1024**2
N = 32768
LIMIT = 64
ARMS = ('arrow-full', 'arrow-pruned', 'duckdb-parquet', 'duckdb-table')
FIELDS = ('group', 'node', 'node_id', 'sequence', 'index', 'observed_ns', 'body', 'attributes')
WHEELS = (
    ('duckdb', '1.4.3', 'duckdb-1.4.3-cp314-cp314-manylinux_2_26_x86_64.manylinux_2_28_x86_64.whl',
     'https://files.pythonhosted.org/packages/c6/5f/87e43af2e4a0135f9675449563e7c2f9b6f1fe6a2d1691c96b091f3904dd/',
     '1b35491db98ccd11d151165497c084a9d29d3dc42fc80abea2715a6c861ca43d', 20497138),
    ('pyarrow', '22.0.0', 'pyarrow-22.0.0-cp314-cp314-manylinux_2_28_x86_64.whl',
     'https://files.pythonhosted.org/packages/55/fc/4945896cc8638536ee787a3bd6ce7cec8ec9acf452d78ec39ab328efa0a1/',
     '6dda1ddac033d27421c20d7a7943eec60be44e0db4e079f33cc5af3b8280ccde', 47737765),
)


def dump(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + '\n')


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def footprint(path):
    total = 0
    for p in path.rglob('*'):
        if p.is_symlink():
            raise RuntimeError('linked scratch/evidence rejected')
        if p.is_file():
            total += max(p.stat().st_size, p.stat().st_blocks * 512)
    return total


def admission(work, out):
    if footprint(work) >= 8 * 1024**3 or footprint(out) >= 100 * MIB:
        raise RuntimeError('8GiB scratch or100MiB evidence boundary reached; preserve full state')
    if shutil.disk_usage(work).free < 16 * 1024**3:
        raise RuntimeError('16GiB data-drive free reserve unavailable')


def io_state():
    return {k: int(v) for k, v in (line.split(':') for line in Path('/proc/self/io').read_text().splitlines())}


def rss():
    return next(int(line.split()[1]) for line in Path('/proc/self/status').read_text().splitlines()
                if line.startswith('VmRSS:'))


def measured(operation):
    before_io, before_rss = io_state(), rss()
    cpu, wall = time.process_time_ns(), time.perf_counter_ns()
    value = operation()
    elapsed, cpu_elapsed = time.perf_counter_ns() - wall, time.process_time_ns() - cpu
    after_io = io_state()
    return value, {'wall_ns': elapsed, 'process_cpu_ns': cpu_elapsed,
                   'rss_before_kib': before_rss, 'rss_after_kib': rss(),
                   'process_hwm_kib': resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
                   'proc_io_delta': {k: after_io[k] - before_io[k] for k in before_io}}


def source_rows(width):
    rows = []
    for i in range(N):
        prefix = f'{i:04x}' + ('S!x' if i < N // 1024 else '---')
        prefix += ('R?y' if i < N // 64 else '---') + ('C%z' if i < N // 2 else '---') + 'λ['
        filler = hashlib.shake_256(f'42:{i}'.encode()).hexdigest(32)
        body = prefix + (filler * ((width - 16 + 63) // 64))[:width - 16]
        assert len(body.encode('utf-8')) == width
        rows.append((i // 8192, 'node-λ', (i % 4).to_bytes(16, 'big'),
                     i // 128 + 1, i % 128, i // 2, body, '{"source":"sweep"}'))
    return rows


def expected_rows(rows, query):
    # Independent Python literal predicate and explicit contract key; no Arrow/SQL/index use.
    selected = [r for r in rows if query['from'] <= r[5] < query['to'] and query['contains'] in r[6]]
    selected.sort(key=lambda r: (r[5], r[2], r[3], r[4]))
    return selected[:LIMIT], len(selected)


def verify_answer(actual, expected):
    if actual != expected:
        raise RuntimeError('independent source-row answer mismatch: content/order/window/limit')


def controls():
    a = (0, 'node-λ', bytes(16), 1, 0, 10, 'λ[.*', '{}')
    b = (0, 'node-λ', bytes(16), 1, 1, 11, 'λ[.*', '{}')
    rejected = []
    for name, bad in [('missing', [a]), ('duplicate', [a, a]), ('ordering', [b, a]),
                      ('changed_body', [a[:-2] + ('changed', '{}'), b]),
                      ('exclusive_upper_boundary', [a, b, b[:-3] + (12, 'λ[.*', '{}')])]:
        try:
            verify_answer(bad, [a, b])
        except RuntimeError:
            rejected.append(name)
        else:
            raise RuntimeError('answer checker accepted injected defect')
    expected, _ = expected_rows([a, b], {'from': 10, 'to': 12, 'contains': '['})
    verify_answer(expected, [a, b])
    # A representative false-negative group-pruning defect loses a real match.
    try:
        verify_answer([], expected)
    except RuntimeError:
        rejected.append('false_negative_group_pruning')
    else:
        raise RuntimeError('answer checker accepted false-negative pruning defect')
    return {'origin': 'new independent lab checker, fixed two-row UTF8/metachar fixture',
            'rejected': rejected, 'valid_literal_control': True}


def gzip_bytes(path, raw):
    with path.open('wb') as output:
        with gzip.GzipFile(fileobj=output, mode='wb', mtime=0, filename='') as stream:
            stream.write(raw)
    with gzip.open(path, 'rb') as stream:
        if stream.read() != raw:
            raise RuntimeError('gzip exact readback mismatch')


def query_definitions():
    end = N // 2
    return [{'name': name, 'from': low, 'to': high, 'contains': needle}
            for name, low, high, needle in (
                ('absent', 0, end, 'MISSING'), ('selective', 0, end, 'S!x'),
                ('rare', 0, end, 'R?y'), ('common', 0, end, 'C%z'),
                ('all', 0, end, ''), ('narrow-time', end // 2, end // 2 + end // 16, ''))]


def worker(args):
    import duckdb
    import pyarrow as pa
    import pyarrow.compute as pc
    import pyarrow.parquet as pq
    pa.set_cpu_count(1)
    pa.set_io_thread_count(1)
    cell = json.loads(args.cell)
    trial = args.trial
    construction = {}
    rows, construction['generate_source'] = measured(lambda: source_rows(cell['width']))
    logical = sorted(rows, key=lambda r: (r[5], r[2], r[3], r[4]))
    physical = list(logical)
    if cell['locality'] == 'mixed':
        random.Random(42).shuffle(physical)
    raw = ('\n'.join(json.dumps([*r[:2], r[2].hex(), *r[3:]], ensure_ascii=False,
                               separators=(',', ':')) for r in logical) + '\n').encode()
    _, construction['source_write_readback'] = measured(lambda: gzip_bytes(trial / 'source.jsonl.gz', raw))
    schema = pa.schema([pa.field(n, t, nullable=False) for n, t in zip(FIELDS,
        (pa.uint64(), pa.string(), pa.binary(16), pa.uint64(), pa.uint32(),
         pa.int64(), pa.string(), pa.string()))])
    table, construction['arrow_conversion'] = measured(lambda: pa.Table.from_arrays(
        [pa.array([r[i] for r in physical], type=schema.types[i]) for i in range(8)], schema=schema))
    parquet = trial / 'logs.parquet'
    _, construction['parquet_write'] = measured(lambda: pq.write_table(table, parquet,
        row_group_size=cell['row_group'], compression='zstd', compression_level=3,
        write_statistics=True, use_dictionary=True))
    pf = pq.ParquetFile(parquet)
    readback = [tuple(row[n] for n in FIELDS) for row in pf.read(use_threads=False).to_pylist()]
    verify_answer(readback, physical)
    def build_index():
        index = []
        for offset in range(0, N, cell['row_group']):
            group = physical[offset:offset + cell['row_group']]
            trigrams = set()
            for row in group:
                body = row[6].encode()
                trigrams.update(body[i:i + 3] for i in range(len(body) - 2))
            index.append({'min': min(r[5] for r in group), 'max': max(r[5] for r in group),
                          'trigrams': trigrams})
        return index
    index, construction['exact_trigram_index_build'] = measured(build_index)
    _, construction['index_serialize_write_readback'] = measured(lambda: gzip_bytes(trial / 'trigrams.json.gz', json.dumps([
        {'min': g['min'], 'max': g['max'], 'trigrams_hex': sorted(t.hex() for t in g['trigrams'])}
        for g in index], separators=(',', ':')).encode()))
    connection = duckdb.connect(str(trial / 'ingested.duckdb'))
    connection.execute("SET threads=1")
    connection.execute("SET memory_limit='1GB'")
    connection.execute("SET temp_directory='" + str(trial / 'duck-temp').replace("'", "''") + "'")
    connection.execute('SET autoinstall_known_extensions=false')
    connection.execute('SET autoload_known_extensions=false')
    _, construction['duckdb_ingest'] = measured(lambda: connection.execute(
        'CREATE TABLE logs AS SELECT * FROM read_parquet(?)', [str(parquet)]))
    _, construction['duckdb_checkpoint'] = measured(lambda: connection.execute('CHECKPOINT'))
    metadata = []
    for i in range(pf.metadata.num_row_groups):
        group = pf.metadata.row_group(i)
        metadata.append({'rows': group.num_rows,
            'compressed_column_bytes': sum(group.column(j).total_compressed_size for j in range(8)),
            'time_min': group.column(5).statistics.min, 'time_max': group.column(5).statistics.max})
        if metadata[-1]['time_min'] != index[i]['min'] or metadata[-1]['time_max'] != index[i]['max']:
            raise RuntimeError('actual Parquet time bounds differ from candidate index')
    (trial / 'answers').mkdir()
    measurements, verdicts, plans = [], [], {}
    def run_arm(arm, query):
        chosen = list(range(len(index)))
        if arm.startswith('arrow'):
            def execute():
                nonlocal chosen
                if arm == 'arrow-pruned':
                    needle = query['contains'].encode()
                    grams = {needle[i:i + 3] for i in range(len(needle) - 2)}
                    chosen = [i for i, group in enumerate(index) if group['max'] >= query['from']
                        and group['min'] < query['to'] and grams.issubset(group['trigrams'])]
                decoded = pf.read_row_groups(chosen, use_threads=False)
                mask = pc.and_(pc.greater_equal(decoded['observed_ns'], query['from']),
                               pc.less(decoded['observed_ns'], query['to']))
                if query['contains']:
                    mask = pc.and_(mask, pc.match_substring(decoded['body'], query['contains']))
                selected = decoded.filter(mask)
                return selected.sort_by([(k, 'ascending') for k in
                    ('observed_ns', 'node_id', 'sequence', 'index')]).slice(0, LIMIT)
            selected, native = measured(execute)
            answer, projection = measured(lambda: [tuple(r[n] for n in FIELDS) for r in selected.to_pylist()])
        else:
            relation = 'read_parquet(?)' if arm == 'duckdb-parquet' else 'logs'
            sql = ('SELECT ' + ','.join('"' + name + '"' for name in FIELDS) + ' FROM ' + relation +
                   ' WHERE observed_ns>=? AND observed_ns<? AND contains(body,?)'
                   ' ORDER BY observed_ns,node_id,sequence,"index" LIMIT 64')
            params = ([str(parquet)] if arm == 'duckdb-parquet' else []) + [
                query['from'], query['to'], query['contains']]
            if arm not in plans:
                plans[arm] = connection.execute('EXPLAIN ' + sql, params).fetchall()
            _, native = measured(lambda: connection.execute(sql, params))
            answer, projection = measured(connection.fetchall)
        return answer, {'native_query': native, 'python_result_projection': projection,
            'combined_wall_ns': native['wall_ns'] + projection['wall_ns'],
            'combined_cpu_ns': native['process_cpu_ns'] + projection['process_cpu_ns'],
            'selected_row_groups': chosen if arm.startswith('arrow') else None,
            'candidate_compressed_column_bytes': sum(metadata[i]['compressed_column_bytes'] for i in chosen)
                if arm.startswith('arrow') else None,
            'native_decoded_bytes': None,
            'bytes_limitation': 'proc IO is OS accounting, not decoder bytes; Arrow candidate bytes are metadata sums.'}
    # Exercise the actual candidate selector, not only an already-missing answer.
    selective = next(q for q in query_definitions() if q['name'] == 'selective')
    expected, _ = expected_rows(logical, selective)
    position = next(i for i in range(len(index)) if any(
        selective['contains'] in row[6] for row in physical[
            i * cell['row_group']:(i + 1) * cell['row_group']]))
    original = index[position]
    pruning_controls = []
    for defect in ('omit_matching_group', 'corrupt_matching_trigram'):
        changed = {'min': original['min'], 'max': original['max'],
                   'trigrams': set(original['trigrams'])}
        if defect == 'omit_matching_group':
            changed['max'] = -1
        else:
            changed['trigrams'].remove(b'S!x')
        vector = {'origin': 'actual arrow-pruned group selector before measured queries',
                  'defect': defect, 'group': position, 'query': selective,
                  'original_min': original['min'], 'original_max': original['max'],
                  'mutated_min': changed['min'], 'mutated_max': changed['max'],
                  'removed_trigrams_hex': sorted(t.hex() for t in original['trigrams'] - changed['trigrams']),
                  'expected_rows': [[*r[:2], r[2].hex(), *r[3:]] for r in expected]}
        index[position] = changed
        try:
            answer, metrics = run_arm('arrow-pruned', selective)
            vector.update(actual_rows=[[*r[:2], r[2].hex(), *r[3:]] for r in answer],
                          selected_groups=metrics['selected_row_groups'])
            try:
                verify_answer(answer, expected)
            except RuntimeError as error:
                vector.update(rejected=True, error=str(error))
            else:
                vector['rejected'] = False
                pruning_controls.append(vector)
                dump(trial / 'group-pruning-controls.json', pruning_controls)
                raise RuntimeError('actual pruning checker accepted injected index defect')
            pruning_controls.append(vector)
            dump(trial / 'group-pruning-controls.json', pruning_controls)
        finally:
            index[position] = original
    restored, _ = run_arm('arrow-pruned', selective)
    verify_answer(restored, expected)
    for qi, query in enumerate(query_definitions()):
        expected, matches = expected_rows(logical, query)
        for iteration in range(4):
            shift = (qi + iteration + cell['repeat']) % len(ARMS)
            order = ARMS[shift:] + ARMS[:shift]
            for arm in order:
                answer, metrics = run_arm(arm, query)
                verify_answer(answer, expected)
                answer_bytes = json.dumps([[*r[:2], r[2].hex(), *r[3:]] for r in answer],
                    ensure_ascii=False, separators=(',', ':')).encode()
                answer_digest = hashlib.sha256(answer_bytes).hexdigest()
                answer_path = trial / 'answers' / (answer_digest + '.json.gz')
                if not answer_path.exists():
                    gzip_bytes(answer_path, answer_bytes)
                measurements.append({'arm': arm, 'query': query, 'matching_rows': matches,
                    'iteration': iteration, 'order': list(order), **metrics})
                verdicts.append({'arm': arm, 'query': query['name'], 'iteration': iteration,
                                 'exact_rows': len(answer), 'passed': True,
                                 'answer_raw_sha256': answer_digest, 'answer_raw_bytes': len(answer_bytes)})
                dump(trial / 'measurements.json', measurements)
                dump(trial / 'verdicts.json', verdicts)
    # Engine-level UTF8 and literal regex metacharacter semantics, separate from timings.
    literal_controls = []
    for needle in ('λ', '[', '.*'):
        query = {'from': 0, 'to': N // 2, 'contains': needle}
        expected, _ = expected_rows(logical, query)
        for arm in ARMS:
            answer, _ = run_arm(arm, query)
            verify_answer(answer, expected)
            literal_controls.append({'arm': arm, 'literal': needle, 'passed': True})
    connection.close()
    amortization = []
    for baseline, candidate, costs in (
        ('arrow-full', 'arrow-pruned', ('exact_trigram_index_build', 'index_serialize_write_readback')),
        ('duckdb-parquet', 'duckdb-table', ('duckdb_ingest', 'duckdb_checkpoint'))):
        for query in query_definitions():
            owned = [m for m in measurements if m['arm'] == baseline and m['query'] == query and m['iteration'] > 0]
            built = [m for m in measurements if m['arm'] == candidate and m['query'] == query and m['iteration'] > 0]
            for metric, cost_metric in (('combined_wall_ns', 'wall_ns'), ('combined_cpu_ns', 'process_cpu_ns')):
                saving = statistics.median(m[metric] for m in owned) - statistics.median(m[metric] for m in built)
                cost = sum(construction[c][cost_metric] for c in costs)
                amortization.append({'baseline': baseline, 'candidate': candidate, 'query': query['name'],
                    'metric': metric, 'construction_cost_ns': cost, 'reuse_median_saving_ns': saving,
                    'queries_to_repay_incremental_cost': math.ceil(cost / saving) if saving > 0 else None,
                    'model': 'Same-query repeat median, same fresh process/cell; incremental construction only. '
                             'Nonpositive savings have no finite repayment; no refill/update measurement.'})
    dump(trial / 'result.json', {'cell': cell, 'versions': {'duckdb': duckdb.__version__, 'pyarrow': pa.__version__},
        'construction': construction, 'row_groups': metadata, 'literal_controls': literal_controls,
        'checker_controls': controls(), 'plans_first_query_only': plans,
        'actual_group_pruning_controls': pruning_controls,
        'amortization': amortization,
        'artifacts': {p.name: {'sha256': sha(p), 'bytes': p.stat().st_size}
                      for p in trial.iterdir() if p.is_file() and p.name != 'result.json'},
        'measurements': len(measurements), 'verdicts': len(verdicts),
        'limitations': ['No Fabric execution: native fixture does not expose matched locality/query cells.',
            'Exact trigram sets are an explanatory lab index, not Fabric Bloom filters.',
            'Only one thread per engine; no OS cache flush; repeat calls reuse process/connection.',
            'Random hex64-byte filler repeats: compressible synthetic bodies, not production corpus.',
            'Installed wheel releases are distinct from previously inspected upstream source HEADs.',
            'Native timing includes library bindings; Python result conversion is reported separately.',
            'RSS high-water is cumulative process scope; no allocation ownership or service-memory claim.']})


def child(argv, env, stdout, stderr, deadline, work, out):
    command = {'argv': argv, 'started': True, 'exit': None}
    with stdout.open('wb') as so, stderr.open('wb') as se:
        process = subprocess.Popen(argv, env=env, cwd=ROOT, stdout=so, stderr=se, start_new_session=True)
        try:
            while process.poll() is None:
                admission(work, out)
                if time.monotonic() >= deadline:
                    raise RuntimeError('registered sweep deadline exhausted')
                time.sleep(.1)
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=10)
            command['exit'] = process.returncode
            dump(stdout.with_suffix('.command.json'), command)
    if process.returncode:
        raise RuntimeError(f'child exit{process.returncode}; full scratch preserved')


def retained_copy(source, target):
    shutil.copy2(source, target)
    if sha(source) != sha(target) or source.read_bytes() != target.read_bytes():
        raise RuntimeError('retained artifact exact readback mismatch')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path)
    parser.add_argument('--seconds', type=int, default=1100)
    parser.add_argument('--worker', action='store_true')
    parser.add_argument('--cell')
    parser.add_argument('--trial', type=Path)
    args = parser.parse_args()
    require_limits()
    if args.worker:
        worker(args)
        return
    if not args.out or not 1 <= args.seconds <= 1100:
        parser.error('fresh --out and deadline<=1100 required')
    if sys.version_info[:2] != (3, 14) or os.uname().machine != 'x86_64':
        raise RuntimeError('pinned cp314 x86_64 wheel platform required')
    inherited_affinity = sorted(os.sched_getaffinity(0))
    os.sched_setaffinity(0, inherited_affinity[:2])
    if sorted(os.sched_getaffinity(0)) != inherited_affinity[:2]:
        raise RuntimeError('two-CPU affinity enforcement unavailable')
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    storage = Path('/run/media/kmosoti/data/FabricO11y/scratch')
    if not scratch.is_relative_to(storage) or not Path(os.environ['TMPDIR']).resolve().is_relative_to(storage):
        raise RuntimeError('launcher-owned data-drive scratch required')
    out = args.out.resolve()
    if out.exists() or not out.is_relative_to(ROOT / 'docs/experiments/benchmarks/data'):
        raise RuntimeError('fresh repository evidence directory required')
    work = scratch / ('query-sweep-' + out.name)
    if work.exists():
        raise RuntimeError('owned scratch exists')
    work.mkdir()
    out.mkdir(parents=True)
    (work / 'owned').write_text(str(out))
    (work / 'wheels').mkdir()
    (out / 'objects').mkdir()
    began, status = time.monotonic(), 'interrupted'
    deadline = began + args.seconds
    try:
        admission(work, out)
        selected = [Path(__file__).resolve(), ROOT / 'crates/fabric-server/src/segment.rs',
            ROOT / 'tools/qualification/query_oracle.py', ROOT / 'tools/resource_group.py',
            ROOT / 'docs/experiments/benchmarks/cross-system-query-sweep-proposal.md']
        wrapper = ROOT / 'tools/bench/labs/cross_system/run_sweep_job.py'
        if wrapper.is_file():
            selected.append(wrapper)
        manifest = {str(p.relative_to(ROOT)): {'sha256': sha(p), 'bytes': p.stat().st_size} for p in selected}
        with tarfile.open(out / 'sources.tar.gz', 'w:gz') as archive:
            for p in selected:
                archive.add(p, arcname=str(p.relative_to(ROOT)), recursive=False)
        with tarfile.open(out / 'sources.tar.gz', 'r:gz') as archive:
            for name, receipt in manifest.items():
                if archive.extractfile(name).read() != (ROOT / name).read_bytes():
                    raise RuntimeError('source archive readback mismatch')
        dump(out / 'source-manifest.json', manifest)
        gzip_bytes(out / 'tracked-working-tree.diff.gz', subprocess.check_output(['git', 'diff', '--binary', 'HEAD'], cwd=ROOT))
        environment = {'argv': sys.argv, 'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
            'python': sys.version, 'uname': list(os.uname()), 'scratch': str(work),
            'versions': {'uv': subprocess.check_output(['uv', '--version'], text=True).strip()},
            'wheel_pins': WHEELS, 'cpu_affinity': sorted(os.sched_getaffinity(0)),
            'inherited_cpu_affinity': inherited_affinity,
            'limits': {'scratch_bytes': 8 * 1024**3, 'retained_bytes': 100 * MIB, 'seconds': args.seconds}}
        dump(out / 'environment.json', environment)
        for name, version, filename, base, digest, size in WHEELS:
            path = work / 'wheels' / filename
            with urllib.request.urlopen(base + filename, timeout=35) as response, path.open('wb') as stream:
                total = 0
                while chunk := response.read(1024**2):
                    total += len(chunk)
                    if total > size or time.monotonic() >= deadline:
                        raise RuntimeError('wheel bound/deadline exceeded')
                    stream.write(chunk)
                    admission(work, out)
            if total != size or sha(path) != digest:
                raise RuntimeError('pinned wheel length/hash mismatch')
            dump(out / (name + '-wheel-verification.json'), {'name': name, 'version': version,
                'url': base + filename, 'bytes': total, 'sha256': sha(path), 'expected_sha256': digest,
                'verified': True, 'successful_cleanup': 'scratch wheel removed only after complete sweep'})
        requirements = work / 'requirements.txt'
        requirements.write_text(''.join(f'{work / "wheels" / filename} --hash=sha256:{digest}\n'
            for _, _, filename, _, digest, _ in WHEELS))
        child(['uv', 'pip', 'install', '--python', sys.executable, '--target', str(work / 'packages'),
               '--no-deps', '--no-index', '--require-hashes', '-r', str(requirements)],
              dict(os.environ, UV_CACHE_DIR=str(work / 'uv-cache')), out / 'install.out', out / 'install.err', deadline, work, out)
        env = dict(os.environ, PYTHONPATH=str(work / 'packages'), PYTHONNOUSERSITE='1',
                   OMP_NUM_THREADS='1', OPENBLAS_NUM_THREADS='1')
        results = []
        for width in (16, 1024):
            for group in (1024, 8192):
                for locality in ('clustered', 'mixed'):
                    identity = None
                    for repeat in (0, 1):
                        cell = {'records': N, 'width': width, 'row_group': group,
                                'locality': locality, 'repeat': repeat, 'seed': 42}
                        label = f'b{width}-g{group}-{locality}-r{repeat}'
                        trial, evidence = work / label, out / label
                        trial.mkdir()
                        evidence.mkdir()
                        child(['/usr/bin/time', '-o', str(evidence / 'process.json'), '-f',
                            '{"wall_seconds":%e,"user_cpu_seconds":%U,"system_cpu_seconds":%S,"max_rss_kib":%M,"exit":%x}',
                            sys.executable, str(Path(__file__).resolve()), '--worker', '--trial', str(trial),
                            '--cell', json.dumps(cell)], env, evidence / 'worker.out', evidence / 'worker.err', deadline, work, out)
                        result = json.loads((trial / 'result.json').read_text())
                        if result['versions'] != {'duckdb': '1.4.3', 'pyarrow': '22.0.0'} or result['verdicts'] != 96:
                            raise RuntimeError('version/measurement coverage mismatch')
                        current = {n: sha(trial / n) for n in ('source.jsonl.gz', 'logs.parquet', 'trigrams.json.gz')}
                        if identity is not None and current != identity:
                            raise RuntimeError('fresh repetition input/index identity drift')
                        identity = current
                        for name, digest in current.items():
                            target = out / 'objects' / digest
                            if not target.exists():
                                retained_copy(trial / name, target)
                            elif target.read_bytes() != (trial / name).read_bytes():
                                raise RuntimeError('deduplicated object differs')
                        answer_objects = {}
                        for source in (trial / 'answers').iterdir():
                            raw_digest = source.name.split('.')[0]
                            with gzip.open(source, 'rb') as stream:
                                answer_raw = stream.read()
                            if hashlib.sha256(answer_raw).hexdigest() != raw_digest:
                                raise RuntimeError('answer artifact decoded hash mismatch')
                            digest = sha(source)
                            target = out / 'objects' / digest
                            if not target.exists():
                                retained_copy(source, target)
                            elif target.read_bytes() != source.read_bytes():
                                raise RuntimeError('deduplicated answer differs')
                            answer_objects[raw_digest] = {'object_sha256': digest, 'raw_bytes': len(answer_raw)}
                        dump(evidence / 'answer-objects.json', answer_objects)
                        dump(evidence / 'input-objects.json', current)
                        for name in ('measurements.json', 'verdicts.json', 'result.json', 'group-pruning-controls.json'):
                            retained_copy(trial / name, evidence / name)
                        results.append({'cell': cell, 'measurements': result['measurements'],
                                        'verdicts': result['verdicts'], 'objects': current})
                        dump(out / 'results.json', results)
                        admission(work, out)
                        if any(sha(ROOT / name) != receipt['sha256'] for name, receipt in manifest.items()):
                            raise RuntimeError('frozen source changed during sweep')
                        shutil.rmtree(trial)
        if sum(r['verdicts'] for r in results) != 1536:
            raise RuntimeError('complete sweep answer count differs from1536')
        status = 'passed'
    except BaseException as error:
        status = 'interrupted' if isinstance(error, KeyboardInterrupt) else 'failed'
        dump(out / 'failure.json', {'status': status, 'error': repr(error), 'full_scratch_preserved': str(work)})
        raise
    finally:
        size = footprint(work)
        if status == 'passed':
            if (work / 'owned').read_text() != str(out):
                raise RuntimeError('scratch ownership mismatch')
            shutil.rmtree(work)
        dump(out / 'cleanup.json', {'status': status, 'scratch': str(work), 'removed': not work.exists(),
            'scratch_bytes_before_cleanup': size, 'retained_bytes': footprint(out),
            'elapsed_seconds': time.monotonic() - began,
            'preservation': 'full worker/scratch state on failure; retained exact source/Parquet/index on success; '
                            'ingested DuckDB files reproducible from retained inputs, DDL and version/hash receipts'})
    print(json.dumps({'status': status, 'evidence': str(out)}))


if __name__ == '__main__':
    main()
