#!/usr/bin/env python3
"""Bounded prepared-scan ablation on immutable retained Vortex files."""
import argparse
import gzip
import hashlib
import importlib.metadata
import json
import math
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import sys
import tarfile
import time
import urllib.request

import query_sweep as q
import vortex_sweep as v

ARMS = ('new-scan', 'prepared-scan')


def admission(work, out):
    if q.footprint(work) >= 512 * q.MIB or q.footprint(out) >= 16 * q.MIB:
        raise RuntimeError('512MiB scratch/16MiB retained bound reached')
    if shutil.disk_usage(work).free < 16 * 1024**3:
        raise RuntimeError('16GiB free reserve required')


def worker(args):
    import pyarrow as pa
    import pyarrow.compute as pc
    import vortex as vx
    import vortex.expr as ve
    pa.set_cpu_count(1)
    pa.set_io_thread_count(1)
    cell = json.loads(args.cell)
    trial = args.trial
    for field in ('source', 'vortex', 'wildcard'):
        if q.sha(Path(cell[field])) != cell[field + '_sha256']:
            raise RuntimeError('immutable retained input changed')
    with gzip.open(cell['source'], 'rt') as stream:
        logical = [tuple([*r[:2], bytes.fromhex(r[2]), *r[3:]]) for r in map(json.loads, stream)]
    if len(logical) != q.N:
        raise RuntimeError('source cardinality drift')
    schema = pa.schema([pa.field(name, dtype, nullable=False) for name, dtype in zip(q.FIELDS,
        (pa.uint64(), pa.string(), pa.binary(), pa.uint64(), pa.uint32(), pa.int64(), pa.string(), pa.string()))])
    def canonical(array):
        arrow = array.to_arrow_array()
        if not pa.types.is_struct(arrow.type):
            raise RuntimeError('selected output must be struct')
        table = (pa.Table.from_batches([], schema=pa.schema(list(arrow.type))) if len(arrow) == 0
                 else pa.Table.from_struct_array(arrow))
        return table.cast(schema)
    def rows(table):
        return [tuple(r[n] for n in q.FIELDS) for r in table.to_pylist()]
    def expression(query):
        expr = (ve.column('observed_ns') >= query['from']) & (ve.column('observed_ns') < query['to'])
        if v.safe_like(query['contains']):
            expr = expr & ve.like(ve.column('body'), '%' + query['contains'] + '%')
        return expr
    def exact(table, query):
        mask = pc.and_(pc.greater_equal(table['observed_ns'], query['from']), pc.less(table['observed_ns'], query['to']))
        if query['contains']:
            mask = pc.and_(mask, pc.match_substring(table['body'], query['contains']))
        return table.filter(mask).sort_by([(name, 'ascending') for name in
            ('observed_ns', 'node_id', 'sequence', 'index')]).slice(0, q.LIMIT)
    vf, opened = q.measured(lambda: vx.open(cell['vortex']))
    # Required API failure is preserved; no substitute implementation.
    if not callable(getattr(vf, 'to_repeated_scan', None)):
        raise RuntimeError('pinned wheel lacks to_repeated_scan')
    queries = q.query_definitions()
    expressions = {query['name']: expression(query) for query in queries}
    plans, preparation = {}, {}
    for query in queries:
        name = query['name']
        plans[name], preparation[name] = q.measured(lambda: vf.to_repeated_scan(expr=expressions[name]))
    def execute(arm, query):
        name = query['name']
        selected, scan = q.measured(lambda: (vf.scan(expr=expressions[name]) if arm == ARMS[0]
                                            else plans[name].execute()).read_all())
        decoded, conversion = q.measured(lambda: canonical(selected))
        result, filtering = q.measured(lambda: exact(decoded, query))
        answer, projection = q.measured(lambda: rows(result))
        phases = dict(scan=scan, selected_arrow_decode=conversion, exact_filter_order_limit=filtering,
                      python_projection=projection)
        return answer, dict(phases=phases, combined_wall_ns=sum(p['wall_ns'] for p in phases.values()),
                            combined_cpu_ns=sum(p['process_cpu_ns'] for p in phases.values()))
    # Reuse the retained two-row actual wildcard counterexample without writing a format.
    control = vx.open(cell['wildcard'])
    bad = rows(canonical(control.to_repeated_scan(expr=ve.like(ve.column('body'), '%C%z%')).execute().read_all()))
    expected = [tuple([*r[:2], bytes.fromhex(r[2]), *r[3:]]) for r in cell['wildcard_expected']]
    rejected = False
    try:
        q.verify_answer(bad, expected)
    except RuntimeError:
        rejected = True
    fallback_query = {'from': 0, 'to': q.N // 2, 'contains': 'C%z'}
    recovered = rows(exact(canonical(control.to_repeated_scan(expr=expression(fallback_query)).execute().read_all()), fallback_query))
    q.verify_answer(recovered, expected)
    q.dump(trial / 'wildcard-control.json', {'bad': json.loads(v.canonical(bad)), 'expected': cell['wildcard_expected'],
        'recovered': json.loads(v.canonical(recovered)), 'rejected': rejected})
    if not rejected or len(bad) != 2:
        raise RuntimeError('actual wildcard defect was not rejected')
    measurements, verdicts = [], []
    (trial / 'answers').mkdir()
    for qi, query in enumerate(queries):
        expected, matches = q.expected_rows(logical, query)
        for iteration in range(4):
            shift = (qi + iteration + cell['repeat']) % 2
            order = ARMS[shift:] + ARMS[:shift]
            for arm in order:
                answer, metrics = execute(arm, query)
                raw = v.canonical(answer)
                digest = hashlib.sha256(raw).hexdigest()
                q.gzip_bytes(trial / 'answers' / (digest + '.json.gz'), raw)
                verdicts.append(dict(arm=arm, query=query['name'], iteration=iteration,
                    answer_raw_sha256=digest, answer_raw_bytes=len(raw), passed=False))
                q.dump(trial / 'verdicts.json', verdicts)
                q.verify_answer(answer, expected)
                verdicts[-1]['passed'] = True
                measurements.append(dict(arm=arm, query=query, iteration=iteration, matching_rows=matches,
                    order=list(order), **metrics))
                q.dump(trial / 'measurements.json', measurements)
                q.dump(trial / 'verdicts.json', verdicts)
    literal_controls = []
    for needle in ('λ', '[', '.*', '_', '\\', 'x' * 254, 'x' * 255):
        query = {'name': 'control', 'from': 0, 'to': q.N // 2, 'contains': needle}
        expressions['control'] = expression(query)
        plans['control'] = vf.to_repeated_scan(expr=expressions['control'])
        expected, _ = q.expected_rows(logical, query)
        for arm in ARMS:
            answer, _ = execute(arm, query)
            q.verify_answer(answer, expected)
            literal_controls.append(dict(needle=needle, arm=arm, passed=True))
    models = []
    for query in queries:
        name = query['name']
        for metric, phase_metric in (('combined_wall_ns', 'wall_ns'), ('combined_cpu_ns', 'process_cpu_ns')):
            medians = [statistics.median(m[metric] for m in measurements if m['arm'] == arm
                and m['query']['name'] == name and m['iteration'] > 0) for arm in ARMS]
            saving = medians[0] - medians[1]
            cost = preparation[name][phase_metric]
            models.append(dict(query=name, metric=metric, baseline_median_ns=medians[0],
                prepared_median_ns=medians[1], preparation_ns=cost, saving_ns=saving,
                queries_to_repay=math.ceil(cost / saving) if saving > 0 else None))
    q.dump(trial / 'result.json', dict(cell=cell, versions={name: importlib.metadata.version(name) for name, *_ in v.WHEELS},
        open=opened, preparation=preparation, models=models, checker_controls=q.controls(),
        literal_controls=literal_controls, measurements=len(measurements)))
    for field in ('source', 'vortex', 'wildcard'):
        if q.sha(Path(cell[field])) != cell[field + '_sha256']:
            raise RuntimeError('input changed during worker')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--inputs', type=Path)
    parser.add_argument('--out', type=Path)
    parser.add_argument('--seconds', type=int, default=120)
    parser.add_argument('--worker', action='store_true')
    parser.add_argument('--trial', type=Path)
    parser.add_argument('--cell')
    args = parser.parse_args()
    q.require_limits()
    if args.worker:
        worker(args)
        return
    if not args.inputs or not args.out or not 1 <= args.seconds <= 120:
        parser.error('--inputs/--out required and seconds<=120')
    if sys.version_info[:2] != (3, 14) or os.uname().machine != 'x86_64':
        raise RuntimeError('pinned cp314 x86_64 platform required')
    os.sched_setaffinity(0, sorted(os.sched_getaffinity(0))[:2])
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    if not scratch.is_relative_to(Path('/run/media/kmosoti/data/FabricO11y/scratch')):
        raise RuntimeError('data-drive scratch required')
    inputs, out = args.inputs.resolve(), args.out.resolve()
    if out.exists() or not out.is_relative_to(q.ROOT / 'docs/experiments/benchmarks/data'):
        raise RuntimeError('fresh repository evidence path required')
    if json.loads((inputs / 'cleanup.json').read_text())['status'] != 'passed':
        raise RuntimeError('successful immutable Vortex inputs required')
    selected = [item for item in json.loads((inputs / 'results.json').read_text()) if item['cell']['repeat'] == 0]
    if (len(selected) != 4 or {(r['cell']['width'], r['cell']['locality']) for r in selected}
            != {(w, locality) for w in (16, 1024) for locality in ('clustered', 'mixed')}
            or any(r['cell']['records'] != q.N or r['cell']['row_group'] != 8192
                   or r['cell']['seed'] != 42 for r in selected)):
        raise RuntimeError('four source configurations required')
    work = scratch / ('vortex-plan-' + out.name)
    work.mkdir()
    out.mkdir(parents=True)
    (work / 'owned').write_text(str(out))
    (work / 'wheels').mkdir()
    (out / 'objects').mkdir()
    v.admission = admission
    began, status = time.monotonic(), 'interrupted'
    deadline = began + args.seconds
    try:
        admission(work, out)
        files = [Path(__file__).resolve(), Path(q.__file__).resolve(), Path(v.__file__).resolve(),
            q.ROOT / 'tools/resource_group.py', q.ROOT / 'docs/experiments/benchmarks/cross-system-vortex-plan-reuse-proposal.md']
        manifest = {str(p.relative_to(q.ROOT)): dict(sha256=q.sha(p), bytes=p.stat().st_size) for p in files}
        with tarfile.open(out / 'sources.tar.gz', 'w:gz') as archive:
            for p in files:
                archive.add(p, arcname=str(p.relative_to(q.ROOT)), recursive=False)
        with tarfile.open(out / 'sources.tar.gz', 'r:gz') as archive:
            for name in manifest:
                if archive.extractfile(name).read() != (q.ROOT / name).read_bytes():
                    raise RuntimeError('source exact archive readback failed')
        q.dump(out / 'source-manifest.json', manifest)
        q.dump(out / 'environment.json', dict(argv=sys.argv, python=sys.version, cpu_affinity=sorted(os.sched_getaffinity(0)),
            wheel_pins=v.WHEELS, inputs=str(inputs), inputs_results_sha256=q.sha(inputs / 'results.json'),
            revision=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=q.ROOT, text=True).strip(),
            uv=subprocess.check_output(['uv', '--version'], text=True).strip()))
        for name, version, filename, base, digest, size in v.WHEELS:
            path = work / 'wheels' / filename
            with urllib.request.urlopen(base + filename, timeout=30) as response, path.open('wb') as stream:
                count = 0
                while chunk := response.read(q.MIB):
                    count += len(chunk)
                    if count > size or time.monotonic() >= deadline:
                        raise RuntimeError('download pin/deadline exceeded')
                    stream.write(chunk)
                    admission(work, out)
            if count != size or q.sha(path) != digest:
                raise RuntimeError('wheel pin mismatch')
            q.dump(out / (name + '-wheel.json'), dict(sha256=digest, bytes=count, url=base + filename, verified=True))
        requirements = work / 'requirements.txt'
        requirements.write_text(''.join(f'{work / "wheels" / filename} --hash=sha256:{digest}\n' for _, _, filename, _, digest, _ in v.WHEELS))
        env = dict(os.environ)
        env.pop('UV_CACHE_DIR', None)
        q.dump(out / 'installer-environment.json', dict(tmpdir=env['TMPDIR'], cache='disabled', link_mode='copy'))
        v.child(['uv', '--no-cache', 'pip', 'install', '--link-mode', 'copy', '--python', sys.executable,
            '--target', str(work / 'packages'), '--no-deps', '--no-index', '--require-hashes', '-r', str(requirements)],
            env, out / 'install.out', out / 'install.err', deadline, work, out)
        api_receipts = {}
        for relative in ('file.py', 'scan.py', '_lib/file.pyi', '_lib/scan.pyi'):
            source = work / 'packages' / 'vortex' / relative
            target = out / 'wheel-api' / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            q.retained_copy(source, target)
            api_receipts[relative] = dict(sha256=q.sha(source), bytes=source.stat().st_size)
        q.dump(out / 'wheel-api-receipts.json', api_receipts)
        env.update(PYTHONPATH=str(work / 'packages'), PYTHONNOUSERSITE='1', OMP_NUM_THREADS='1',
                   RAYON_NUM_THREADS='1', TOKIO_WORKER_THREADS='2')
        results = []
        for item in selected:
            base_cell = item['cell']
            old = inputs / f'b{base_cell["width"]}-{base_cell["locality"]}-r0'
            old_result = json.loads((old / 'result.json').read_text())
            cell = {name: base_cell[name] for name in ('records', 'width', 'row_group', 'locality', 'seed')}
            cell.update(source=base_cell['source'], source_sha256=base_cell['source_sha256'],
                wildcard_expected=old_result['wildcard_control']['expected_literal_rows'])
            for field, filename in (('vortex', 'logs.vortex'), ('wildcard', 'wildcard-control.vortex')):
                cell[field], cell[field + '_sha256'] = str(old / filename), old_result['artifacts'][filename]['sha256']
            for repeat in (0, 1):
                cell['repeat'] = repeat
                trial = work / f'b{cell["width"]}-{cell["locality"]}-r{repeat}'
                evidence = out / trial.name
                trial.mkdir()
                evidence.mkdir()
                v.child(['/usr/bin/time', '-o', str(evidence / 'process.json'), '-f',
                    '{"wall_seconds":%e,"user_cpu_seconds":%U,"system_cpu_seconds":%S,"max_rss_kib":%M,"exit":%x}',
                    sys.executable, str(Path(__file__).resolve()), '--worker', '--trial', str(trial), '--cell', json.dumps(cell)],
                    env, evidence / 'worker.out', evidence / 'worker.err', deadline, work, out)
                result = json.loads((trial / 'result.json').read_text())
                if result['measurements'] != 48 or result['versions'] != {name: version for name, version, *_ in v.WHEELS}:
                    raise RuntimeError('coverage/versions differ from registration')
                for p in trial.iterdir():
                    if p.is_file():
                        q.retained_copy(p, evidence / p.name)
                objects = {}
                for p in (trial / 'answers').iterdir():
                    with gzip.open(p, 'rb') as stream:
                        raw = stream.read()
                    raw_digest = p.name.split('.')[0]
                    if hashlib.sha256(raw).hexdigest() != raw_digest:
                        raise RuntimeError('answer exact raw readback failed')
                    digest = q.sha(p)
                    target = out / 'objects' / digest
                    if not target.exists():
                        q.retained_copy(p, target)
                    elif target.read_bytes() != p.read_bytes():
                        raise RuntimeError('deduplicated answer mismatch')
                    objects[raw_digest] = digest
                q.dump(evidence / 'answer-objects.json', objects)
                results.append(dict(cell=dict(cell), measurements=48, result_sha256=q.sha(evidence / 'result.json')))
                q.dump(out / 'results.json', results)
                admission(work, out)
                if any(q.sha(q.ROOT / name) != receipt['sha256'] for name, receipt in manifest.items()):
                    raise RuntimeError('frozen source changed during experiment')
                shutil.rmtree(trial)
        if len(results) != 8:
            raise RuntimeError('eight fresh children required')
        status = 'passed'
    except BaseException as error:
        status = 'interrupted' if isinstance(error, KeyboardInterrupt) else 'failed'
        q.dump(out / 'failure.json', dict(status=status, error=repr(error), scratch=str(work)))
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
                raise RuntimeError('owned scratch identity drift')
            shutil.rmtree(work)
        q.dump(out / 'cleanup.json', dict(status=status, removed=not work.exists(), scratch=str(work),
            scratch_bytes=size, accounting_error=accounting_error, retained_bytes=q.footprint(out),
            elapsed_seconds=time.monotonic() - began))
        if accounting_error and sys.exc_info()[0] is None:
            raise RuntimeError('accounting failure; full scratch preserved')


if __name__ == '__main__':
    main()
