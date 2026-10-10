#!/usr/bin/env python3
"""Bounded log query census; reuse the frozen completion grader and native probe.

Invoke only through tools/resource_group.py after registering the selected cells.
No cache flush is performed. Native first/repeat names describe History lifetimes,
not cold/warm OS cache measurements. Whole-probe resource costs include fixture
construction, tail commits, Segment construction and all continuation drains.
"""
import argparse
import gzip
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tarfile
import time

ROOT = Path(__file__).resolve().parents[4]
spec = importlib.util.spec_from_file_location(
    'completion_profile', ROOT / 'tools/bench/labs/completion/profile.py')
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)
grader = profile.grader
COUNTS = (128, 2048, 8192)
WIDTHS = (16, 1024)
LIMIT = 64
REPEATS = 3
RAW_CASE_LIMIT = 128 * 1024**2
RAW_CASE_STOP = 120 * 1024**2
BORROWED_EVIDENCE_LIMIT = 114 * 1024**2
ABLATION_ORDERS = (
    ('plain-owned', 'plain-borrowed', 'counted-borrowed', 'counted-owned'),
    ('plain-borrowed', 'plain-owned', 'counted-owned', 'counted-borrowed'),
    ('counted-owned', 'counted-borrowed', 'plain-borrowed', 'plain-owned'),
    ('counted-borrowed', 'counted-owned', 'plain-owned', 'plain-borrowed'),
)


def check_space(work, out, evidence_limit=None):
    profile.check_space(work, out)
    if evidence_limit is not None and grader.footprint(out) >= evidence_limit:
        raise RuntimeError('borrowed ablation reached114MiB new evidence bound; preserve state')


def fixture_bound(count, width):
    """Bound ordinary expected artifacts; this is admission, not a query oracle.

    Reproduce body bytes solely to charge JSON escaping and actual common matches.
    A corrupt native writer can exceed this prediction, so enforce a monitored
    stop threshold too and preserve any overrun rather than truncate evidence.
    """
    body_sizes, common_sizes = [], []
    for j in range(count):
        body = bytearray(f'bench-{j:04} '.encode())
        state = 42 ^ (j + 1)
        while len(body) < width:
            state ^= (state << 13) & ((1 << 64) - 1)
            state ^= state >> 7
            state ^= (state << 17) & ((1 << 64) - 1)
            body.append(ord('R') if j % 2 == 0 else ord('!') + state % 94)
        row_bytes = len(json.dumps(body.decode(), separators=(',', ':')).encode()) + 512
        body_sizes.append(row_bytes)
        if b'RRRR' in body:
            common_sizes.append(row_bytes)
    # 16 chains per shape, exact common cardinality, all broad rows, one selective.
    chain_bytes = 16 * (sum(body_sizes) + sum(common_sizes) + max(body_sizes))
    # Measured wrappers duplicate at most 64 selected rows plus continuation sentinel.
    measured_bytes = 64 * 65 * max(body_sizes)
    custody_bytes = 12 * count * (width + 256)
    fixed_bytes = 8 * 1024**2  # state headers, manifests, filters, envelopes and phase ledgers
    return {'expected_raw_case_upper_bytes': chain_bytes + measured_bytes + custody_bytes + fixed_bytes,
            'raw_case_limit_bytes': RAW_CASE_LIMIT, 'monitored_stop_bytes': RAW_CASE_STOP,
            'components': {'chains': chain_bytes, 'measured_wrappers': measured_bytes,
                           'custody_and_native_state': custody_bytes, 'fixed_allowance': fixed_bytes},
            'assumption': 'Expected fixture schema and bounded native writer; monitored limit is not a disk quota.'}


def run_probe(argv, env, trial, evidence, deadline, work, out, evidence_limit=None):
    if time.monotonic() >= deadline:
        raise RuntimeError('registered census deadline exhausted')
    with (trial / 'timings.jsonl').open('wb') as so, (evidence / 'probe.err').open('wb') as se:
        child = subprocess.Popen(argv, cwd=ROOT, env=env, stdout=so, stderr=se, start_new_session=True)
        try:
            while child.poll() is None:
                check_space(work, out, evidence_limit)
                if grader.footprint(trial) >= RAW_CASE_STOP:
                    raise RuntimeError('raw case reached120MiB stop threshold; preserve full state')
                if time.monotonic() >= deadline:
                    raise RuntimeError('registered census deadline exhausted')
                time.sleep(.05)
        finally:
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=10)
            dump(evidence / 'native-execution.json', {'argv': argv, 'exit': child.returncode,
                'raw_case_bytes': grader.footprint(trial),
                'raw_case128MiB_bound_observed': grader.footprint(trial) <= RAW_CASE_LIMIT})
    return child.returncode


def dump(path, value):
    profile.dump(path, value)


def bounds(count, width):
    # Each of 16 shape/layout/plan combinations has four measured calls.
    # Empty and selective shapes fit one page; common conservatively <= all rows.
    per_chain = count // LIMIT + 1
    return {'record_count': count, 'body_bytes': width, 'groups': count // 128,
            'source_bytes': count * (width + 1), 'measured_wrappers': 64,
            'complete_chains': 64,
            'chain_pages_upper': 16 * (2 + 2 * per_chain),
            'page_count_guard': count // LIMIT + 2,
            'raw_answer_ledger_native_cap_bytes': 256 * 1024**2,
            'raw_single_chain_native_cap_bytes': 256 * 1024**2,
            'raw_case_admission': fixture_bound(count, width)}


def archive_sources(out):
    """Freeze dirty tracked and untracked source bytes, not only HEAD identity."""
    files = set(ROOT.glob('Cargo.*'))
    files.update(ROOT.glob('rust-toolchain*'))
    for folder in ('.cargo', 'crates', 'xtask', 'tools/qualification', 'tools/bench/labs/completion',
                   'tools/bench/labs/cross_system'):
        files.update(p for p in (ROOT / folder).rglob('*') if p.is_file()
                     and p.suffix in ('.rs', '.toml', '.py', '.json', '.md', '.proto', '.lock'))
    files.add(ROOT / 'tools/bench/run_responsibility_isolation.py')
    files.add(ROOT / 'tools/resource_group.py')
    manifest = {}
    with tarfile.open(out / 'sources.tar.gz', 'w:gz') as archive:
        for path in sorted(files):
            relative = str(path.relative_to(ROOT))
            manifest[relative] = {'sha256': grader.sha(path), 'bytes': path.stat().st_size}
            archive.add(path, arcname=relative, recursive=False)
    diff = subprocess.check_output(['git', 'diff', '--binary', 'HEAD'], cwd=ROOT)
    with gzip.open(out / 'tracked-working-tree.diff.gz', 'wb') as stream:
        stream.write(diff)
    with (out / 'tracked-working-tree.diff.gz').open('rb') as stream:
        diff_verified = verify_source_gzip(stream, diff)
    dump(out / 'tracked-diff-verification.json', diff_verified)
    dump(out / 'source-manifest.json', manifest)
    with (out / 'sources.tar.gz').open('rb') as stream:
        verified = verify_source_archive(stream, manifest, lambda name: (ROOT / name).read_bytes())
    dump(out / 'source-archive-verification.json', verified)
    return manifest


def verify_source_archive(stream, manifest, source_bytes):
    """Read back every archived member against both exact source and manifest."""
    seen, size = set(), 0
    with tarfile.open(fileobj=stream, mode='r:gz') as archive:
        for member in archive:
            if not member.isfile() or member.name in seen or member.name not in manifest:
                raise RuntimeError('source archive member type, duplicate or name mismatch')
            raw = archive.extractfile(member).read()
            receipt = manifest[member.name]
            if (raw != source_bytes(member.name) or member.size != receipt['bytes']
                    or len(raw) != receipt['bytes']
                    or hashlib.sha256(raw).hexdigest() != receipt['sha256']):
                raise RuntimeError('source archive differs from exact source or manifest')
            seen.add(member.name)
            size += len(raw)
    if seen != set(manifest):
        raise RuntimeError('source archive member coverage mismatch')
    return {'members': len(seen), 'raw_bytes': size,
            'exact_source_bytes_compared': True, 'manifest_hashes_checked': True}


def verify_source_gzip(stream, expected):
    with gzip.GzipFile(fileobj=stream, mode='rb') as archive:
        raw = archive.read(len(expected) + 1)
    if raw != expected:
        raise RuntimeError('source gzip differs from exact original source')
    return {'raw_bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest(),
            'exact_source_bytes_compared': True}


def archive_controls():
    """Deterministic corruption fixtures exercise the new readback boundary."""
    expected = b'bench-0000 RRRRR\n'
    manifest = {'fixture.rs': {'bytes': len(expected),
                              'sha256': hashlib.sha256(expected).hexdigest()}}
    def tar_fixture(entries):
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode='w:gz') as archive:
            for name, raw in entries:
                member = tarfile.TarInfo(name)
                member.size = len(raw)
                archive.addfile(member, io.BytesIO(raw))
        output.seek(0)
        return output
    valid_tar = tar_fixture([('fixture.rs', expected)])
    verify_source_archive(valid_tar, manifest, lambda name: expected)
    verify_source_gzip(io.BytesIO(gzip.compress(expected, mtime=0)), expected)
    rejected = {}
    fixtures = {
        'changed_archive_member': [('fixture.rs', expected.replace(b'RRRRR', b'RRRRS'))],
        'missing_archive_member': [],
        'duplicate_archive_member': [('fixture.rs', expected), ('fixture.rs', expected)],
        'unlisted_archive_member': [('fixture.rs', expected), ('other.rs', expected)],
    }
    for name, entries in fixtures.items():
        try:
            verify_source_archive(tar_fixture(entries), manifest, lambda member: expected)
        except RuntimeError as error:
            rejected[name] = str(error)
        else:
            raise RuntimeError(f'archive readback accepted {name} control')
    for name, compressed in {
        'changed_source_gzip': gzip.compress(expected.replace(b'RRRRR', b'RRRRS'), mtime=0),
        'truncated_source_gzip': gzip.compress(expected, mtime=0)[:-4],
    }.items():
        try:
            verify_source_gzip(io.BytesIO(compressed), expected)
        except (RuntimeError, OSError, EOFError) as error:
            rejected[name] = str(error)
        else:
            raise RuntimeError(f'source readback accepted {name} control')
    return {'origin': 'query_census archive readback boundary; fixed same-length byte mutation',
            'valid_tar_and_gzip_checked': True, 'rejected': rejected}


def check_source_identity(manifest):
    changed = [name for name, receipt in manifest.items()
               if not (ROOT / name).is_file() or grader.sha(ROOT / name) != receipt['sha256']]
    if changed:
        raise RuntimeError(f'archived source changed during census: {changed}')


def phase_summary(path):
    summary = {}
    with path.open() as stream:
        for line in stream:
            row = json.loads(line)
            item = summary.setdefault(row['phase'], {'calls': 0, 'wall_ns': 0,
                                                     'thread_cpu_ns': 0})
            item['calls'] += 1
            item['wall_ns'] += row['wall_ns']
            item['thread_cpu_ns'] += row['thread_cpu_ns']
    return {'inclusive_spans_by_phase': summary,
            'limitation': 'Nested spans overlap; do not sum as exclusive costs. '
                          'Continuation query spans exclude measured first pages.'}


def summarize(trial, evidence, counted):
    timings = [json.loads(line) for line in (trial / 'timings.jsonl').read_text().splitlines()]
    measurements = [r for r in timings if r['stage'].startswith('query_')
                    or r['stage'].startswith('answer_json_serialize_')]
    chain_maps = json.loads((evidence / 'chain-map.json').read_text())
    result = {'first_page_measurements': measurements,
              'whole_probe_resources': json.loads((evidence / 'process-resources.json').read_text()),
              'measured_boundary_vm_hwm_kib': next(
                  r['vm_hwm_kib'] for r in timings if r['stage'] == 'measured_queries_complete'),
              'final_vm_hwm_kib': timings[-1]['vm_hwm_kib'],
              'complete_chains': len(chain_maps),
              'complete_chain_pages': sum(len(c['pages']) for c in chain_maps),
              'complete_chain_raw_bytes': sum(c['raw_bytes'] for c in chain_maps),
              'cache_control': 'unavailable: OS cache not flushed; History reset for first calls',
              'pure_full_chain_latency': 'unavailable: plain native continuation helper has no enclosing timer',
              'whole_probe_scope': 'fixture generation, custody, Segment construction, first pages, '
                                   'serialization and complete continuation drains; excludes Python grading',
              'observer_scope': 'counted combines allocator and phase probes; no independent observer ablation'}
    if counted:
        for name in ('phases', 'continuation-phases'):
            result[name] = phase_summary(trial / (name + '.jsonl'))
    dump(evidence / 'census.json', result)
    return result


def allocation_pairs(results):
    """Pair counted first-page calls by stage and occurrence; keep every delta.

    This reports the registered allocation hypothesis without making an unmet
    prediction a correctness failure. Native warm labels are History reuse only.
    """
    grouped = {}
    for result in results:
        if result['variant'].startswith('counted-'):
            grouped.setdefault((result['records'], result['body_bytes']), {})[
                result['variant']] = result['metrics']['first_page_measurements']
    pairs = []
    for (count, width), variants in sorted(grouped.items()):
        def indexed(rows):
            seen, indexed_rows = {}, {}
            for row in rows:
                if not row['stage'].startswith('query_'):
                    continue
                stage = row['stage']
                occurrence = seen.get(stage, 0)
                seen[stage] = occurrence + 1
                indexed_rows[(stage, occurrence)] = row
            return indexed_rows
        owned = indexed(variants['counted-owned'])
        borrowed = indexed(variants['counted-borrowed'])
        if owned.keys() != borrowed.keys():
            raise RuntimeError('counted owned/borrowed measurement coverage differs')
        for key, row in owned.items():
            other = borrowed[key]
            saving = (row['allocation']['cumulative_requested_bytes']
                      - other['allocation']['cumulative_requested_bytes'])
            pairs.append({'records': count, 'body_bytes': width,
                'stage': key[0], 'occurrence': key[1],
                'owned_requested_bytes': row['allocation']['cumulative_requested_bytes'],
                'borrowed_requested_bytes': other['allocation']['cumulative_requested_bytes'],
                'requested_bytes_saved': saving,
                'owned_cpu_ns': row['cpu_ns'], 'borrowed_cpu_ns': other['cpu_ns'],
                'owned_wall_ns': row['wall_ns'], 'borrowed_wall_ns': other['wall_ns']})
    target = [p for p in pairs if p['records'] == 2048 and p['body_bytes'] == 1024
              and p['stage'].startswith(('query_segment_scan_selective_',
                                         'query_segment_walk_selective_'))]
    return {'paired_counted_calls': pairs, 'target_calls': len(target),
            'target_threshold_bytes': 1024**2,
            'target_each_call_saves_at_least_threshold': bool(target) and all(
                p['requested_bytes_saved'] >= 1024**2 for p in target),
            'interpretation': 'Allocation diagnostic; protocol determines hypothesis verdict. '
                              'No CPU improvement threshold; counted timing includes observers.'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path)
    parser.add_argument('--counts', type=int, nargs='+', default=[128, 2048])
    parser.add_argument('--widths', type=int, nargs='+', default=list(WIDTHS))
    parser.add_argument('--build-seconds', type=int, default=600)
    parser.add_argument('--run-seconds', type=int)
    parser.add_argument('--borrowed-ablation', action='store_true')
    parser.add_argument('--controls', action='store_true')
    args = parser.parse_args()
    if args.run_seconds is None:
        args.run_seconds = 500 if args.borrowed_ablation else 900
    if (len(set(args.counts)) != len(args.counts) or any(n not in COUNTS for n in args.counts)
            or len(set(args.widths)) != len(args.widths) or any(n not in WIDTHS for n in args.widths)):
        parser.error('cells must be unique subsets of counts128/2048/8192 and widths16/1024')
    if not (0 < args.build_seconds <= 900 and 0 < args.run_seconds <= 900
            and args.build_seconds + args.run_seconds <= 1700):
        parser.error('build+run/grading <=1700s, each <=900s; outer launcher deadline remains1800s')
    if args.borrowed_ablation and (args.counts != [128, 2048] or args.widths != list(WIDTHS)
                                  or args.build_seconds + args.run_seconds > 1200):
        parser.error('borrowed ablation requires128/2048 counts,16/1024 widths, build+run <=1200s')
    variants_to_build = (('plain-owned', 'plain-borrowed', 'counted-owned', 'counted-borrowed')
                         if args.borrowed_ablation else ('plain', 'counted'))
    evidence_limit = BORROWED_EVIDENCE_LIMIT if args.borrowed_ablation else None
    profile.require_limits()
    admitted = [bounds(n, w) for n in args.counts for w in args.widths]
    if any(b['raw_case_admission']['expected_raw_case_upper_bytes'] >= RAW_CASE_STOP for b in admitted):
        parser.error('selected cell exceeds120MiB expected raw-case admission; register a smaller selection')
    if args.controls:
        print(json.dumps({'unchanged_grader': profile.harness_controls(),
                          'archive_readback': archive_controls()}))
        return
    if args.out is None:
        parser.error('--out required')
    tmp = Path(os.environ['TMPDIR']).resolve()
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    if not tmp.is_relative_to(profile.STORAGE / 'scratch') or not scratch.is_relative_to(profile.STORAGE / 'scratch'):
        raise RuntimeError('launcher-owned data-drive scratch required')
    out = args.out.resolve()
    if out.exists() or not out.is_relative_to(ROOT / 'docs/experiments/benchmarks/data'):
        raise RuntimeError('fresh repository evidence directory required')
    if not Path('/usr/bin/time').is_file():
        raise RuntimeError('/usr/bin/time required for per-process CPU and peak RSS')
    work = scratch / ('cross-system-query-census-' + out.name)
    if work.exists():
        raise RuntimeError('owned census scratch already exists')
    out.mkdir(parents=True)
    work.mkdir()
    (work / 'owned').write_text(str(out))
    (work / 'bin').mkdir()
    (out / 'objects').mkdir()
    commands, results = [], []
    status, began = 'interrupted', time.monotonic()
    try:
        check_space(work, out, evidence_limit)
        manifest = archive_sources(out)
        dump(out / 'planned-bounds.json', admitted)
        dump(out / 'harness-controls.json', profile.harness_controls())
        dump(out / 'archive-controls.json', archive_controls())
        environment = {'command': sys.argv,
            'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
            'source_archive': 'sources.tar.gz', 'sources': manifest,
            'storage': str(work), 'uname': list(os.uname()),
            'cpu_affinity': sorted(os.sched_getaffinity(0)),
            'bounds': {'scratch_bytes': profile.SCRATCH_LIMIT, 'evidence_bytes': profile.EVIDENCE_LIMIT,
                       'free_reserve_bytes': profile.FREE_RESERVE},
            'build_settings': {k: os.environ.get(k) for k in ('RUSTFLAGS', 'CARGO_TARGET_DIR', 'CARGO_BUILD_JOBS')},
            'binary_features': {v: ['responsibility-alloc-probe', 'phase-probe']
                               if v.startswith('counted') else [] for v in variants_to_build},
            'interpretation': 'Diagnostic census only; no cold/warm, exclusive allocation ownership, '
                              'service memory, deployment capacity or pure full-chain latency claim.'}
        dump(out / 'environment.json', environment)
        if args.borrowed_ablation:
            environment.update(borrowed_ablation=True,
                binary_compile_env={v: {'FABRIC_BORROWED_LOG_EXPERIMENT':
                    '1' if v.endswith('-borrowed') else '0'} for v in variants_to_build},
                cell_orders=ABLATION_ORDERS, new_evidence_limit_bytes=evidence_limit,
                planned_complete_chains=1024, variants_per_cell=4,
                planned_chain_pages_upper=4 * sum(b['chain_pages_upper'] for b in admitted),
                compile_switch='History::borrowed_logs_enabled: option_env!("FABRIC_BORROWED_LOG_EXPERIMENT") == "1"')
            dump(out / 'environment.json', environment)
        build_deadline = began + args.build_seconds
        for variant in variants_to_build:
            check_source_identity(manifest)
            argv = ['cargo', 'build', '--offline', '--locked', '--release', '-p',
                    'fabric-server', '--example', 'completion_query_probe']
            if variant.startswith('counted'):
                argv += ['--features', 'responsibility-alloc-probe,phase-probe']
            command = {'argv': argv, 'exit': None, 'status': 'started'}
            build_env = dict(os.environ)
            if args.borrowed_ablation:
                command['unset_compile_env'] = ['FABRIC_RUN_MIB_EXPERIMENT',
                                                'FABRIC_SPILL_WORKSPACE_EXPERIMENT']
                for name in command['unset_compile_env']:
                    build_env.pop(name, None)
                command['compile_env'] = environment['binary_compile_env'][variant]
                build_env.update(command['compile_env'])
            commands.append(command)
            dump(out / 'commands.json', commands)
            code = profile.run_child(argv, build_env, out / (variant + '-build.out'),
                                     out / (variant + '-build.err'), build_deadline, work, out)
            command.update(exit=code, status='returned')
            dump(out / 'commands.json', commands)
            if code:
                raise RuntimeError(f'{variant} build exit {code}')
            shutil.copy2(Path(os.environ['CARGO_TARGET_DIR']) / 'release/examples/completion_query_probe',
                         work / 'bin' / variant)
            check_source_identity(manifest)
            check_space(work, out, evidence_limit)
        environment['binaries'] = {v: grader.sha(work / 'bin' / v) for v in variants_to_build}
        environment['build_seconds'] = time.monotonic() - began
        dump(out / 'environment.json', environment)
        deadline = min(began + 1700, time.monotonic() + args.run_seconds)
        for cell_index, (count, width) in enumerate((n, w) for n in args.counts for w in args.widths):
            identity = None
            variants = (ABLATION_ORDERS[cell_index] if args.borrowed_ablation else
                        ('plain', 'counted') if cell_index % 2 == 0 else ('counted', 'plain'))
            for variant in variants:
                counted = variant.startswith('counted')
                check_source_identity(manifest)
                label = f'n{count}-b{width}-{variant}'
                trial, evidence = work / label, out / label
                trial.mkdir()
                evidence.mkdir()
                env = {k: v for k, v in os.environ.items() if not k.startswith('BENCH_')}
                env.update(BENCH_RECORDS=str(count), BENCH_BODY_SIZE=str(width),
                    BENCH_ORDER='shuffled', BENCH_QUERY_REPEATS=str(REPEATS), BENCH_OBSERVER='minimal',
                    BENCH_PREFLIGHT='0', BENCH_QUERY_ROTATION='0', BENCH_QUERY_LIMIT=str(LIMIT),
                    BENCH_PHASES='1' if counted else '0')
                resources = evidence / 'process-resources.json'
                argv = ['/usr/bin/time', '-o', str(resources), '-f',
                    '{"wall_seconds":%e,"user_cpu_seconds":%U,"system_cpu_seconds":%S,'
                    '"max_rss_kib":%M,"major_faults":%F,"minor_faults":%R,"exit":%x}',
                    str(work / 'bin' / variant), str(trial), 'query', str(width)]
                command = {'argv': argv, 'exit': None, 'status': 'started',
                           'bench_env': {k: v for k, v in env.items() if k.startswith('BENCH_')}}
                commands.append(command)
                dump(out / 'commands.json', commands)
                code = run_probe(argv, env, trial, evidence, deadline, work, out, evidence_limit)
                command.update(exit=code, status='returned')
                dump(out / 'commands.json', commands)
                if code:
                    raise RuntimeError(f'{label} probe exit {code}')
                raw_size = grader.footprint(trial)
                dump(evidence / 'raw-case-resource.json', {'raw_bytes': raw_size,
                    'within128MiB': raw_size <= RAW_CASE_LIMIT, 'predicted': fixture_bound(count, width)})
                if raw_size > RAW_CASE_LIMIT:
                    raise RuntimeError('raw case exceeded128MiB; preserve state and record overrun')
                fixture = profile.collect(trial, evidence, 'counted' if counted else 'plain',
                                          'full', deadline, limit=LIMIT)
                for key, expected in {'records': count, 'body_bytes': width, 'query_warm_repeats': REPEATS,
                                      'query_limit': LIMIT, 'timestamp_order': 'shuffled', 'seed': 42}.items():
                    if fixture[key] != expected:
                        raise RuntimeError(f'fixture {key} drift: {fixture[key]} != {expected}')
                if args.borrowed_ablation and fixture['borrowed_logs'] != variant.endswith('-borrowed'):
                    raise RuntimeError(f'{label} compile-time borrowed flag drift')
                if args.borrowed_ablation and fixture['catalog_shared']:
                    raise RuntimeError('borrowed ablation unexpectedly enabled shared catalog')
                current = {k: fixture[k] for k in ('source_sha256', 'timestamp_ranks_sha256', 'encoded_batch_bytes')}
                if identity is not None and identity != current:
                    raise RuntimeError('cell binary fixture identity differs')
                identity = current
                grader.compress(trial / 'source.log', evidence / 'source.log.gz')
                with (evidence / 'source.log.gz').open('rb') as stream:
                    source_readback = verify_source_gzip(stream, (trial / 'source.log').read_bytes())
                dump(evidence / 'source-readback.json', source_readback)
                if counted:
                    dump(evidence / 'phase-checks.json', profile.phase_checks(trial))
                    for name in ('phases', 'continuation-phases'):
                        grader.compress(trial / (name + '.jsonl'), evidence / (name + '.jsonl.gz'))
                elif (trial / 'phases.jsonl').exists():
                    raise RuntimeError('plain binary unexpectedly emits phase observer ledger')
                result = {'cell': label, 'metrics': summarize(trial, evidence, counted)}
                if args.borrowed_ablation:
                    result.update(variant=variant, records=count, body_bytes=width)
                results.append(result)
                dump(out / 'results.json', results)
                check_space(work, out, evidence_limit)
                check_source_identity(manifest)
                shutil.rmtree(trial)
        if args.borrowed_ablation:
            dump(out / 'borrowed-allocation-pairs.json', allocation_pairs(results))
            if sum(r['metrics']['complete_chains'] for r in results) != 1024:
                raise RuntimeError('borrowed ablation complete chain count differs from1024')
            check_space(work, out, evidence_limit)
        with (out / 'sources.tar.gz').open('rb') as stream:
            dump(out / 'source-archive-final-verification.json', verify_source_archive(
                stream, manifest, lambda name: (ROOT / name).read_bytes()))
        check_space(work, out, evidence_limit)
        status = 'passed'
    except BaseException as error:
        status = 'interrupted' if isinstance(error, KeyboardInterrupt) else 'failed'
        cases = {p.name: grader.footprint(p) for p in work.iterdir() if p.is_dir() and p.name != 'bin'}
        dump(out / 'failure.json', {'status': status, 'error': repr(error), 'scratch_preserved': str(work),
                                  'raw_cases_bytes': cases,
                                  'raw_case128MiB_bound_observed': all(n <= RAW_CASE_LIMIT for n in cases.values())})
        raise
    finally:
        dump(out / 'commands.json', commands)
        size = grader.footprint(work)
        if status == 'passed':
            if (work / 'owned').read_text() != str(out):
                raise RuntimeError('scratch ownership mismatch')
            shutil.rmtree(work)
        dump(out / 'cleanup.json', {'status': status, 'storage': str(work),
            'removed': not work.exists(), 'scratch_logical_bytes': size,
            'retained_logical_bytes': grader.footprint(out),
            'elapsed_seconds': time.monotonic() - began,
            'failure_preservation': 'owned scratch retained on failure; never replaced with success'})
    print(json.dumps({'status': status, 'evidence': str(out)}))


if __name__ == '__main__':
    main()
