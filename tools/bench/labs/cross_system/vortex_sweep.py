#!/usr/bin/env python3
"""Opt-in Vortex wheel comparison on exact retained query-sweep inputs."""
import argparse
import gzip
import hashlib
import importlib.metadata
import json
import math
import os
from pathlib import Path
import shutil
import signal
import statistics
import subprocess
import sys
import tarfile
import time
import urllib.request

import query_sweep as q

ROOT = q.ROOT
ARMS = ('arrow-parquet-full', 'vortex-full', 'vortex-predicate')
EXTRA_WHEELS = (
    ('vortex-data', '0.87.0', 'vortex_data-0.87.0-cp311-abi3-manylinux_2_17_x86_64.manylinux2014_x86_64.whl',
     'https://files.pythonhosted.org/packages/28/ad/07c86129bcec928153c1f815fd308884804173582fdfb8d7d1876eb0db01/',
     '9525588a85777613b84b058e850fb34bded51d5da094eebfa392cdf693a7f2ea', 44001518),
    ('typing-extensions', '4.15.0', 'typing_extensions-4.15.0-py3-none-any.whl',
     'https://files.pythonhosted.org/packages/18/67/36e9267722cc04a6b9f15c7f3441c2363321a3ea07da7ae0c0707beb2a9c/',
     'f0fa19c6845758ab08074a0cfa8b7aecb71c999ca73d62883bc25cc018c4e548', 44614),
    ('substrait', '0.28.0', 'substrait-0.28.0-py3-none-any.whl',
     'https://files.pythonhosted.org/packages/1a/b8/40c8ac43fe4099f056b84e4652f5e5a8e8c742b5d2850ea1c3dec7ee32b9/',
     'c36c5f424e901534b6150a7cc56fb75d237c6e85ce4c0e95102c16acc8f69414', 42028),
    ('substrait-protobuf', '0.79.0', 'substrait_protobuf-0.79.0-py3-none-any.whl',
     'https://files.pythonhosted.org/packages/0a/27/24e16471dc7b5986cd2af0d5aab9b12347fb835840fb2320b1b2375e53a8/',
     '6f710459569be0b92661e16a2d180a9f2a03853c3ef0154a0c6c8a9aef365ffa', 83741),
    ('substrait-extensions', '0.79.0', 'substrait_extensions-0.79.0-py3-none-any.whl',
     'https://files.pythonhosted.org/packages/f3/2f/a05dd536db76ba9309bb80df559a203614d107ef2381560872fb12370396/',
     '4ca15d059b912e7e030b4c7a5f0a7c434206c7543f699e5f896caf9cb0b47b65', 101699),
    ('protobuf', '6.33.5', 'protobuf-6.33.5-cp39-abi3-manylinux2014_x86_64.whl',
     'https://files.pythonhosted.org/packages/9b/53/a9443aa3ca9ba8724fdfa02dd1887c1bcd8e89556b715cfbacca6b63dbec/',
     'cbf16ba3350fb7b889fca858fb215967792dc125b35c7976ca4818bee3521cf0', 323465),
)
WHEELS = (q.WHEELS[1], *EXTRA_WHEELS)


def admission(work, out):
    if q.footprint(work) >= 8 * 1024**3 or q.footprint(out) >= 128 * q.MIB:
        raise RuntimeError('8GiB scratch/128MiB evidence bound reached; preserve full scratch')
    if shutil.disk_usage(work).free < 16 * 1024**3:
        raise RuntimeError('16GiB data-drive free reserve unavailable')


def child(argv, env, out, err, deadline, work, evidence):
    receipt = {'argv': argv, 'exit': None}
    with out.open('wb') as so, err.open('wb') as se:
        process = subprocess.Popen(argv, cwd=ROOT, env=env, stdout=so, stderr=se, start_new_session=True)
        try:
            while process.poll() is None:
                admission(work, evidence)
                if time.monotonic() >= deadline:
                    raise RuntimeError('registered Vortex deadline exhausted')
                time.sleep(.1)
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=10)
            receipt['exit'] = process.returncode
            q.dump(out.with_suffix('.command.json'), receipt)
    if process.returncode:
        raise RuntimeError(f'Vortex child exit{process.returncode}; full state preserved')


def canonical(rows):
    return json.dumps([[*r[:2], r[2].hex(), *r[3:]] for r in rows],
                      ensure_ascii=False, separators=(',', ':')).encode()


def safe_like(needle):
    # No escape parameter is exposed by the documented Python LIKE API.
    return bool(needle) and len(needle.encode()) <= 254 and not any(c in needle for c in '%_\\')


def worker(args):
    import pyarrow as pa
    import pyarrow.compute as pc
    import pyarrow.parquet as pq
    import vortex as vx
    import vortex.expr as ve
    pa.set_cpu_count(1)
    pa.set_io_thread_count(1)
    trial, cell = args.trial, json.loads(args.cell)
    source, parquet = Path(cell['source']), Path(cell['parquet'])
    if q.sha(source) != cell['source_sha256'] or q.sha(parquet) != cell['parquet_sha256']:
        raise RuntimeError('retained query input hash drift')
    with gzip.open(source, 'rt') as stream:
        logical = [tuple([*r[:2], bytes.fromhex(r[2]), *r[3:]])
                   for r in (json.loads(line) for line in stream)]
    if len(logical) != q.N:
        raise RuntimeError('retained source cardinality drift')
    construction = {}
    table, construction['parquet_decode_for_conversion'] = q.measured(lambda: pq.read_table(parquet, use_threads=False))
    physical = [tuple(r[n] for n in q.FIELDS) for r in table.to_pylist()]
    q.verify_answer(sorted(physical, key=lambda r: (r[5], r[2], r[3], r[4])), logical)
    def widen_identity():
        # Vortex binary dtype does not promise Arrow FixedSizeBinary16 preservation.
        return table.set_column(2, pa.field('node_id', pa.binary(), nullable=False), pc.cast(table['node_id'], pa.binary()))
    adapted, construction['fixed_binary_to_binary_conversion'] = q.measured(widen_identity)
    def canonical_table(array):
        arrow = array.to_arrow_array()
        if not pa.types.is_struct(arrow.type):
            raise RuntimeError('Vortex selected output is not a struct array')
        if len(arrow) == 0:
            # Empty ChunkedArray has no RecordBatch from which Arrow can infer
            # a Table schema. Preserve the actual struct fields, then perform
            # the same checked canonical cast as the nonempty path.
            return pa.Table.from_batches([], schema=pa.schema(list(arrow.type))).cast(adapted.schema)
        return pa.Table.from_struct_array(arrow).cast(adapted.schema)
    path = trial / 'logs.vortex'
    _, construction['vortex_compress_write'] = q.measured(lambda: vx.io.write(adapted, str(path)))
    vf, construction['vortex_open'] = q.measured(lambda: vx.open(str(path)))
    representation, construction['full_representation_readback'] = q.measured(lambda: vf.scan().read_all())
    tree = representation.display_tree()
    representation_nbytes = representation.nbytes
    (trial / 'encoding-tree.txt').write_text(tree)
    readback = canonical_table(representation)
    actual = [tuple(r[n] for n in q.FIELDS) for r in readback.to_pylist()]
    q.verify_answer(actual, physical)
    del representation, readback, actual
    (trial / 'answers').mkdir()

    def exact_arrow(decoded, query):
        mask = pc.and_(pc.greater_equal(decoded['observed_ns'], query['from']),
                       pc.less(decoded['observed_ns'], query['to']))
        if query['contains']:
            mask = pc.and_(mask, pc.match_substring(decoded['body'], query['contains']))
        selected = decoded.filter(mask)
        # Canonical sort requires all matches; a scan limit before this is unsound for mixed physical order.
        return selected.sort_by([(k, 'ascending') for k in
            ('observed_ns', 'node_id', 'sequence', 'index')]).slice(0, q.LIMIT)

    def execute(arm, query):
        route = 'full-exact'
        if arm == 'arrow-parquet-full':
            decoded, scan = q.measured(lambda: pq.read_table(parquet, use_threads=False))
            conversion = None
        else:
            def read():
                nonlocal route
                expression = None
                if arm == 'vortex-predicate':
                    expression = (ve.column('observed_ns') >= query['from']) & (ve.column('observed_ns') < query['to'])
                    if safe_like(query['contains']):
                        expression = expression & ve.like(ve.column('body'), '%' + query['contains'] + '%')
                        route = 'time-and-safe-like'
                    else:
                        route = 'time-pushdown-exact-literal-fallback'
                return vf.scan(expr=expression).read_all()
            selected, scan = q.measured(read)
            decoded, conversion = q.measured(lambda: canonical_table(selected))
        result, exact = q.measured(lambda: exact_arrow(decoded, query))
        answer, projection = q.measured(lambda: [tuple(r[n] for n in q.FIELDS) for r in result.to_pylist()])
        phases = {'representation_scan': scan, 'selected_arrow_decode': conversion,
                  'exact_filter_order_limit': exact, 'python_projection': projection}
        present = [p for p in phases.values() if p is not None]
        return answer, {'route': route, 'phases': phases,
            'combined_wall_ns': sum(p['wall_ns'] for p in present),
            'combined_cpu_ns': sum(p['process_cpu_ns'] for p in present),
            'physical_decoded_bytes': None, 'fsst_dispatch_verified': False}

    # Actual wildcard false positive: raw LIKE accepts Cxxz, while literal C%z does not.
    a, b = logical[0], logical[1]
    control_rows = [a[:6] + ('C%z', a[7]), b[:6] + ('Cxxz', b[7])]
    control_table = pa.Table.from_arrays([pa.array([r[i] for r in control_rows], type=adapted.schema.types[i])
                                         for i in range(8)], schema=adapted.schema)
    control_path = trial / 'wildcard-control.vortex'
    vx.io.write(control_table, str(control_path))
    control_file = vx.open(str(control_path))
    raw_like = canonical_table(control_file.scan(expr=ve.like(ve.column('body'), '%C%z%')).read_all())
    raw_rows = [tuple(r[n] for n in q.FIELDS) for r in raw_like.to_pylist()]
    query = {'from': 0, 'to': q.N // 2, 'contains': 'C%z'}
    expected, _ = q.expected_rows(control_rows, query)
    vector = {'origin': 'actual Vortex LIKE wildcard-vs-literal control', 'query': query,
              'input_rows': json.loads(canonical(control_rows)), 'raw_like_rows': json.loads(canonical(raw_rows)),
              'expected_literal_rows': json.loads(canonical(expected)), 'raw_like_rejected': False}
    try:
        q.verify_answer(raw_rows, expected)
    except RuntimeError as error:
        vector.update(raw_like_rejected=True, error=str(error))
    q.dump(trial / 'wildcard-control.json', vector)
    if len(raw_rows) != 2 or not vector['raw_like_rejected'] or safe_like('C%z'):
        raise RuntimeError('wildcard false-positive control did not reject actual bad adapter')
    fallback = exact_arrow(canonical_table(control_file.scan().read_all()), query)
    q.verify_answer([tuple(r[n] for n in q.FIELDS) for r in fallback.to_pylist()], expected)
    vector['exact_fallback_passed'] = True
    q.dump(trial / 'wildcard-control.json', vector)

    measurements, verdicts = [], []
    for qi, query in enumerate(q.query_definitions()):
        expected, matches = q.expected_rows(logical, query)
        for iteration in range(4):
            shift = (qi + iteration + cell['repeat']) % 3
            order = ARMS[shift:] + ARMS[:shift]
            for arm in order:
                answer, metrics = execute(arm, query)
                raw = canonical(answer)
                digest = hashlib.sha256(raw).hexdigest()
                q.gzip_bytes(trial / 'answers' / (digest + '.json.gz'), raw)
                verdict = {'arm': arm, 'query': query['name'], 'iteration': iteration,
                           'answer_raw_sha256': digest, 'answer_raw_bytes': len(raw), 'passed': False}
                verdicts.append(verdict)
                q.dump(trial / 'verdicts.json', verdicts)
                q.verify_answer(answer, expected)
                verdict['passed'] = True
                measurements.append({'arm': arm, 'query': query, 'iteration': iteration,
                    'matching_rows': matches, 'order': list(order), **metrics})
                q.dump(trial / 'measurements.json', measurements)
                q.dump(trial / 'verdicts.json', verdicts)
    literal_controls = []
    for needle in ('λ', '[', '.*', '_', '\\', 'x' * 254, 'x' * 255):
        query = {'from': 0, 'to': q.N // 2, 'contains': needle}
        expected, _ = q.expected_rows(logical, query)
        for arm in ARMS:
            answer, metrics = execute(arm, query)
            q.verify_answer(answer, expected)
            literal_controls.append({'arm': arm, 'needle': needle, 'route': metrics['route'], 'passed': True})
    repayment = []
    costs = ('parquet_decode_for_conversion', 'fixed_binary_to_binary_conversion', 'vortex_compress_write', 'vortex_open')
    for candidate in ('vortex-full', 'vortex-predicate'):
        for query in q.query_definitions():
            baseline = [m for m in measurements if m['arm'] == ARMS[0] and m['query'] == query and m['iteration'] > 0]
            candidates = [m for m in measurements if m['arm'] == candidate and m['query'] == query and m['iteration'] > 0]
            for metric, costmetric in (('combined_wall_ns', 'wall_ns'), ('combined_cpu_ns', 'process_cpu_ns')):
                saving = statistics.median(m[metric] for m in baseline) - statistics.median(m[metric] for m in candidates)
                cost = sum(construction[c][costmetric] for c in costs)
                repayment.append({'candidate': candidate, 'query': query['name'], 'metric': metric,
                    'incremental_conversion_cost_ns': cost, 'reuse_median_saving_ns': saving,
                    'queries_to_repay': math.ceil(cost / saving) if saving > 0 else None})
    q.dump(trial / 'result.json', {'cell': cell, 'versions': {name: importlib.metadata.version(name) for name, *_ in WHEELS},
        'construction': construction, 'repayment': repayment, 'measurements': len(measurements),
        'checker_controls': q.controls(), 'wildcard_control': vector, 'literal_controls': literal_controls,
        'representation': {'tree': 'encoding-tree.txt', 'fsst_text_present': 'fsst' in tree.lower(),
                           'kernel_dispatch_verified': False, 'file_bytes': path.stat().st_size,
                           'full_array_reported_nbytes': representation_nbytes},
        'artifacts': {p.name: {'sha256': q.sha(p), 'bytes': p.stat().st_size} for p in trial.iterdir() if p.is_file()},
        'limitations': ['No FSST dispatch or decoder-byte claim; encoding tree alone is insufficient.',
            'FixedSizeBinary16 widens to binary before Vortex; logical bytes/identity verified unchanged.',
            'Vortex Arrow string/binary views canonicalize to the ingestion schema inside selected_arrow_decode.',
            'Vortex file layout is library-selected, not matched Parquet row groups.',
            'No OS cache flush; full readback and controls precede timing.',
            'No Fabric production format migration, CR2 nomination, server capacity or engine dominance claim.']})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--inputs', type=Path)
    parser.add_argument('--out', type=Path)
    parser.add_argument('--seconds', type=int, default=900)
    parser.add_argument('--worker', action='store_true')
    parser.add_argument('--trial', type=Path)
    parser.add_argument('--cell')
    args = parser.parse_args()
    q.require_limits()
    if args.worker:
        worker(args)
        return
    if not args.out or not args.inputs or not 1 <= args.seconds <= 900:
        parser.error('--inputs/--out required, seconds<=900')
    if sys.version_info[:2] != (3, 14) or os.uname().machine != 'x86_64':
        raise RuntimeError('pinned CPython3.14 x86_64 required')
    inherited = sorted(os.sched_getaffinity(0))
    os.sched_setaffinity(0, inherited[:2])
    if sorted(os.sched_getaffinity(0)) != inherited[:2]:
        raise RuntimeError('two CPU affinity enforcement failed')
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    if not scratch.is_relative_to(Path('/run/media/kmosoti/data/FabricO11y/scratch')):
        raise RuntimeError('data-drive scratch required')
    inputs, out = args.inputs.resolve(), args.out.resolve()
    if out.exists() or not out.is_relative_to(ROOT / 'docs/experiments/benchmarks/data'):
        raise RuntimeError('fresh repository evidence path required')
    if json.loads((inputs / 'cleanup.json').read_text())['status'] != 'passed':
        raise RuntimeError('complete successful query-sweep inputs required')
    selected = [r for r in json.loads((inputs / 'results.json').read_text())
                if r['cell']['row_group'] == 8192 and r['cell']['repeat'] == 0]
    if len(selected) != 4:
        raise RuntimeError('four exact width/locality representative inputs required')
    work = scratch / ('vortex-sweep-' + out.name)
    if work.exists():
        raise RuntimeError('owned scratch exists')
    work.mkdir()
    out.mkdir(parents=True)
    (work / 'owned').write_text(str(out))
    (work / 'wheels').mkdir()
    (out / 'objects').mkdir()
    (out / 'inputs').mkdir()
    began, status = time.monotonic(), 'interrupted'
    deadline = began + args.seconds
    try:
        admission(work, out)
        source_files = [Path(__file__).resolve(), Path(q.__file__).resolve(), ROOT / 'tools/resource_group.py',
            ROOT / 'docs/experiments/benchmarks/cross-system-vortex-sweep-proposal.md']
        manifest = {str(p.relative_to(ROOT)): {'sha256': q.sha(p), 'bytes': p.stat().st_size} for p in source_files}
        with tarfile.open(out / 'sources.tar.gz', 'w:gz') as archive:
            for p in source_files:
                archive.add(p, arcname=str(p.relative_to(ROOT)), recursive=False)
        with tarfile.open(out / 'sources.tar.gz', 'r:gz') as archive:
            for name in manifest:
                if archive.extractfile(name).read() != (ROOT / name).read_bytes():
                    raise RuntimeError('source archive exact readback failed')
        q.dump(out / 'source-manifest.json', manifest)
        q.dump(out / 'environment.json', {'argv': sys.argv, 'python': sys.version, 'uname': list(os.uname()),
            'uv': subprocess.check_output(['uv', '--version'], text=True).strip(), 'wheel_pins': WHEELS,
            'cpu_affinity': sorted(os.sched_getaffinity(0)), 'inherited_affinity': inherited,
            'inputs': str(inputs), 'inputs_results_sha256': q.sha(inputs / 'results.json'),
            'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()})
        q.gzip_bytes(out / 'tracked-working-tree.diff.gz', subprocess.check_output(['git', 'diff', '--binary', 'HEAD'], cwd=ROOT))
        for name, version, filename, base, digest, size in WHEELS:
            path = work / 'wheels' / filename
            with urllib.request.urlopen(base + filename, timeout=35) as response, path.open('wb') as stream:
                count = 0
                while chunk := response.read(q.MIB):
                    count += len(chunk)
                    if count > size or time.monotonic() >= deadline:
                        raise RuntimeError('bounded wheel download exceeded pin/deadline')
                    stream.write(chunk)
                    admission(work, out)
            if count != size or q.sha(path) != digest:
                raise RuntimeError('wheel hash/length differs from pin')
            q.dump(out / (name + '-wheel.json'), {'url': base + filename, 'sha256': q.sha(path), 'bytes': count, 'verified': True})
        requirements = work / 'requirements.txt'
        requirements.write_text(''.join(f'{work / "wheels" / filename} --hash=sha256:{digest}\n'
            for _, _, filename, _, digest, _ in WHEELS))
        installer_env = dict(os.environ)
        installer_env.pop('UV_CACHE_DIR', None)
        q.dump(out / 'installer-environment.json', {'tmpdir': installer_env['TMPDIR'],
            'cache': 'disabled', 'link_mode': 'copy'})
        child(['uv', '--no-cache', 'pip', 'install', '--link-mode', 'copy',
            '--python', sys.executable, '--target', str(work / 'packages'),
            '--no-deps', '--no-index', '--require-hashes', '-r', str(requirements)],
            installer_env, out / 'install.out', out / 'install.err', deadline, work, out)
        env = dict(os.environ, PYTHONPATH=str(work / 'packages'), PYTHONNOUSERSITE='1',
                   OMP_NUM_THREADS='1', RAYON_NUM_THREADS='1', TOKIO_WORKER_THREADS='2')
        results = []
        for item in selected:
            cell = dict(item['cell'])
            for name, field in (('source.jsonl.gz', 'source'), ('logs.parquet', 'parquet')):
                digest = item['objects'][name]
                old, retained = inputs / 'objects' / digest, out / 'inputs' / digest
                if q.sha(old) != digest:
                    raise RuntimeError('query-sweep object no longer matches receipt')
                if not retained.exists():
                    q.retained_copy(old, retained)
                cell[field], cell[field + '_sha256'] = str(retained), digest
            for repeat in (0, 1):
                cell['repeat'] = repeat
                label = f'b{cell["width"]}-{cell["locality"]}-r{repeat}'
                trial, evidence = work / label, out / label
                trial.mkdir()
                evidence.mkdir()
                child(['/usr/bin/time', '-o', str(evidence / 'process.json'), '-f',
                    '{"wall_seconds":%e,"user_cpu_seconds":%U,"system_cpu_seconds":%S,"max_rss_kib":%M,"exit":%x}',
                    sys.executable, str(Path(__file__).resolve()), '--worker', '--trial', str(trial),
                    '--cell', json.dumps(cell)], env, evidence / 'worker.out', evidence / 'worker.err', deadline, work, out)
                result = json.loads((trial / 'result.json').read_text())
                if result['measurements'] != 72 or result['versions'] != {name: version for name, version, *_ in WHEELS}:
                    raise RuntimeError('measurement/version coverage differs from scope')
                objects = {}
                for p in trial.iterdir():
                    if p.is_file():
                        q.retained_copy(p, evidence / p.name)
                for source in (trial / 'answers').iterdir():
                    with gzip.open(source, 'rb') as stream:
                        raw = stream.read()
                    if hashlib.sha256(raw).hexdigest() != source.name.split('.')[0]:
                        raise RuntimeError('answer raw readback hash differs')
                    digest = q.sha(source)
                    target = out / 'objects' / digest
                    if not target.exists():
                        q.retained_copy(source, target)
                    elif source.read_bytes() != target.read_bytes():
                        raise RuntimeError('deduplicated answer differs')
                    objects[source.name.split('.')[0]] = digest
                q.dump(evidence / 'answer-objects.json', objects)
                results.append({'cell': dict(cell), 'measurements': result['measurements'], 'answer_objects': objects})
                q.dump(out / 'results.json', results)
                admission(work, out)
                if any(q.sha(ROOT / name) != receipt['sha256'] for name, receipt in manifest.items()):
                    raise RuntimeError('frozen source changed during Vortex sweep')
                shutil.rmtree(trial)
        if sum(r['measurements'] for r in results) != 576:
            raise RuntimeError('Vortex answer count differs from576')
        status = 'passed'
    except BaseException as error:
        status = 'interrupted' if isinstance(error, KeyboardInterrupt) else 'failed'
        q.dump(out / 'failure.json', {'status': status, 'error': repr(error), 'full_scratch_preserved': str(work)})
        raise
    finally:
        accounting_error = None
        try:
            size = q.footprint(work)
        except (OSError, RuntimeError) as error:
            size, accounting_error = None, repr(error)
            if status == 'passed':
                status = 'failed'
        if status == 'passed':
            if (work / 'owned').read_text() != str(out):
                raise RuntimeError('scratch ownership mismatch')
            shutil.rmtree(work)
        q.dump(out / 'cleanup.json', {'status': status, 'removed': not work.exists(), 'scratch': str(work),
            'scratch_bytes_before_cleanup': size, 'retained_bytes': q.footprint(out),
            'scratch_accounting_error': accounting_error,
            'elapsed_seconds': time.monotonic() - began})
        if accounting_error and sys.exc_info()[0] is None:
            raise RuntimeError('scratch accounting failed; full state preserved: ' + accounting_error)
    print(json.dumps({'status': status, 'evidence': str(out)}))


if __name__ == '__main__':
    main()
