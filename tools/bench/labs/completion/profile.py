#!/usr/bin/env python3
"""Private first-page allocation screen with complete, separately captured chains."""
import argparse
import copy
import gzip
import hashlib
import json
import os
import re
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import STORAGE, require_limits
sys.path.insert(0, str(ROOT / 'tools/bench'))
import run_responsibility_isolation as grader

EVIDENCE_LIMIT = 256 * 1024**2
SCRATCH_LIMIT = 8 * 1024**3
FREE_RESERVE = 16 * 1024**3


def dump(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def check_space(work, out):
    if grader.footprint(work) > SCRATCH_LIMIT:
        raise RuntimeError('owned profiling scratch exceeds 8GiB')
    evidence_root = next((p for p in (out, *out.parents) if p.name == 'query'), out)
    if grader.footprint(evidence_root) > EVIDENCE_LIMIT:
        raise RuntimeError('query lab evidence exceeds 256MiB; preserve scratch')
    if shutil.disk_usage(STORAGE).free < FREE_RESERVE:
        raise RuntimeError('16GiB data-drive free-space reserve unavailable')


def run_child(argv, env, stdout, stderr, deadline, work, out):
    if time.monotonic() >= deadline:
        raise RuntimeError('registered profiling deadline exhausted')
    with stdout.open('wb') as so, stderr.open('wb') as se:
        child = subprocess.Popen(argv, cwd=ROOT, env=env, stdout=so, stderr=se, start_new_session=True)
        try:
            while child.poll() is None:
                check_space(work, out)
                if time.monotonic() >= deadline:
                    raise RuntimeError('registered profiling deadline exhausted')
                time.sleep(.25)
        finally:
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=10)
    return child.returncode


def retain_bytes(payload, folder, suffix):
    """Deduplicate only after comparing exact bytes, even on matching SHA256."""
    digest = hashlib.sha256(payload).hexdigest()
    path = folder / (digest + suffix + '.gz')
    if path.exists():
        with gzip.open(path, 'rb') as stream:
            if stream.read() != payload:
                raise RuntimeError('SHA256 collision or retained evidence corruption')
    else:
        with gzip.open(path, 'wb', compresslevel=6) as stream:
            stream.write(payload)
    return path.name


def same_batch_records(left, right):
    def semantic(raw):
        rows = [json.loads(line) for line in raw.splitlines()]
        return [{k: v for k, v in row.items() if k != 'received_ns'} for row in rows]
    return semantic(left) == semantic(right)


def ledger_controls():
    original = b'{"hex":"ab","sha256":"batch","label":"fixture","received_ns":1}\n'
    assert same_batch_records(original, original.replace(b'"received_ns":1', b'"received_ns":2'))
    rejected = {}
    for name, bad in {
        'changed_batch': original.replace(b'"ab"', b'"ac"'),
        'changed_hash': original.replace(b'"batch"', b'"wrong"'),
        'changed_label': original.replace(b'"fixture"', b'"other"'),
        'missing_record': b'', 'duplicate_record': original + original,
    }.items():
        assert not same_batch_records(original, bad), name
        rejected[name] = True
    return {'only_received_ns_ignored': True, 'rejected': rejected}


def retain_ledger(raw, catalog):
    frames, reconstructed = [], bytearray()
    for line in raw.splitlines(keepends=True):
        match = re.search(rb'"hex"\s*:\s*"([0-9a-f]+)"', line)
        if match is None:
            raise RuntimeError('native records hexadecimal framing missing')
        prefix, payload, suffix = line[:match.start(1)], match.group(1), line[match.end(1):]
        obj = json.loads(line)
        if hashlib.sha256(bytes.fromhex(payload.decode())).hexdigest() != obj['sha256']:
            raise RuntimeError('record batch digest mismatch')
        name = retain_bytes(payload, catalog, '.batch.hex')
        frames.append({'prefix_hex': prefix.hex(), 'object': name, 'suffix_hex': suffix.hex()})
        with gzip.open(catalog / name, 'rb') as stream:
            reconstructed.extend(prefix + stream.read() + suffix)
    if bytes(reconstructed) != raw:
        raise RuntimeError('lossless records reconstruction disagrees with exact input bytes')
    index = {'format': 'prefix_hex + decompressed ASCII hex object + suffix_hex per frame',
        'raw_sha256': hashlib.sha256(raw).hexdigest(), 'raw_bytes': len(raw), 'frames': frames}
    return retain_bytes(json.dumps(index, separators=(',', ':')).encode(), catalog, '.records-index.json')


def wrapper_parts(line):
    prefix, rest = line.split(b',"answer":', 1)
    payload, suffix = rest.rsplit(b',"population":', 1)
    return prefix + b',"answer":', payload, b',"population":' + suffix


def check_phase_rows(rows):
    calls = sorted((p for p in rows if p['phase'] == 'query_run_inclusive'),
                   key=lambda p: p['start_ns'])
    loaders = [p for p in rows if p['phase'] in
               ('query_scan_source_loading', 'query_walk_source_loading')]
    expected = ['scan'] * 16 + ['walk'] * 16 + ['scan'] * 16 + ['walk'] * 16
    if len(calls) != 64 or len(loaders) != 64:
        raise RuntimeError('expected exactly 64 measured query and loader spans')
    for call, plan in zip(calls, expected):
        nested = [p for p in loaders if p['thread'] == call['thread'] and
                  call['start_ns'] <= p['start_ns'] and
                  p['start_ns'] + p['wall_ns'] <= call['start_ns'] + call['wall_ns']]
        if len(nested) != 1 or nested[0]['phase'] != 'query_' + plan + '_source_loading':
            raise RuntimeError('loader label does not match measured plan')
    return {'calls': len(calls), 'loader_spans': len(loaders),
            'one_matching_loader_per_call': True}


def phase_checks(work):
    """Every measured call gets its own matching, non-overlapping loader label."""
    rows = [json.loads(line) for line in (work / 'phases.jsonl').read_text().splitlines()]
    result = check_phase_rows(rows)
    if not (work / 'continuation-phases.jsonl').exists():
        raise RuntimeError('separate continuation phase ledger missing')
    result['continuation_phases_separate'] = True
    return result


def check_first_page(measured, chain_first):
    if measured != chain_first:
        raise RuntimeError('chain first page differs from measured exact bytes')


def frozen_query(shape, count, *, empty_text=False, limit=10000):
    if not isinstance(empty_text, bool) or not isinstance(limit, int) or isinstance(limit, bool) or not 1 <= limit <= 10000:
        raise RuntimeError('explicit query selector/page limit outside supported bounds')
    query = {'kind': 'logs', 'from_ns': 1_600_000_000_000_000_000,
             'to_ns': 1_600_000_000_000_000_000 + count, 'limit': limit}
    if shape == 'empty':
        if empty_text:
            query['contains'] = '__CR2_ABSENT__'
        else:
            query.update(from_ns=1, to_ns=2)
    elif shape == 'selective':
        query['contains'] = 'bench-0007 '
    elif shape == 'common':
        query['contains'] = 'RRRR'
    elif shape != 'broad':
        raise RuntimeError('unknown frozen query shape')
    return query


def check_query(actual, shape, count, *, empty_text=False, limit=10000):
    if actual != frozen_query(shape, count, empty_text=empty_text, limit=limit):
        raise RuntimeError('frozen query shape drift')


def harness_controls():
    result = {'ledger': ledger_controls(), 'rejected': {}}
    original = b'{"query":{"kind":"logs"},"answer":{"rows":[]},"population":"first","iteration":0}\n'
    prefix, payload, suffix = wrapper_parts(original)
    if prefix + payload + suffix != original:
        raise RuntimeError('lossless wrapper control failed')
    check_first_page(payload, payload)
    try:
        check_first_page(payload, payload + b' ')
    except RuntimeError:
        result['rejected']['changed_first_page_bytes'] = True
    else:
        raise RuntimeError('first-page association accepted changed bytes')
    for empty_text, limit in ((False, 10000), (True, 10000), (True, 100)):
        query = frozen_query('empty', 65536, empty_text=empty_text, limit=limit)
        check_query(query, 'empty', 65536, empty_text=empty_text, limit=limit)
        for field, value in (('contains', 'changed-selector'), ('limit', limit + 1)):
            try:
                check_query(dict(query, **{field: value}), 'empty', 65536, empty_text=empty_text, limit=limit)
            except RuntimeError:
                result['rejected'][f'query_{empty_text}_{limit}_{field}'] = True
            else:
                raise RuntimeError('query shape checker accepted changed selector/limit')
    rows = []
    expected = ['scan'] * 16 + ['walk'] * 16 + ['scan'] * 16 + ['walk'] * 16
    for i, plan in enumerate(expected):
        rows += [{'phase': 'query_run_inclusive', 'thread': 'fixture',
                  'start_ns': i * 100, 'wall_ns': 90},
                 {'phase': 'query_' + plan + '_source_loading', 'thread': 'fixture',
                  'start_ns': i * 100 + 1, 'wall_ns': 80}]
    check_phase_rows(rows)
    for name, bad in (
        ('missing_loader', rows[:-1]),
        ('wrong_plan_loader', [dict(p, phase='query_walk_source_loading')
                              if p['phase'] == 'query_scan_source_loading' else p for p in rows]),
        ('nested_opposite_loader', rows + [dict(rows[1], phase='query_walk_source_loading')]),
    ):
        try:
            check_phase_rows(bad)
        except RuntimeError:
            result['rejected'][name] = True
        else:
            raise RuntimeError(f'phase checker accepted {name}')
    return result


def grade_chains(work, out, expected, population, *, empty_text=False, limit=10000):
    """Adapt the transcript, never the independent complete-pagination oracle."""
    catalog = out.parent / 'objects'
    verdicts, controls, mappings, chain_maps = [], {}, [], []
    expected_files = {f'answer-{kind}-{plan}-{shape}.jsonl' for kind in ('tail', 'segment')
                      for plan in ('scan', 'walk')
                      for shape in ('empty', 'selective', 'common', 'broad')}
    paths = sorted(work.glob('answer-*.jsonl'))
    if {p.name for p in paths} != expected_files:
        raise RuntimeError('measured shape/layout/plan coverage drift')
    actual_chains = set()
    for kind in ('tail', 'segment'):
        records, bodies = grader.decode_records(work / (kind + '-records/records.jsonl'))
        grader.exact(expected, bodies)
        for path in (p for p in paths if p.name.split('-')[1] == kind):
            shape = path.stem.split('-')[3]
            calls_seen = 0
            with path.open('rb') as stream:
                for index, line in enumerate(stream):
                    calls_seen += 1
                    if index > 3:
                        raise RuntimeError('more than four measured calls per shape')
                    prefix, payload, suffix = wrapper_parts(line)
                    obj = json.loads(line)
                    if obj['iteration'] != index or obj['population'] != ('first' if index == 0 else 'warm'):
                        raise RuntimeError('first/warm association drift')
                    check_query(obj['query'], shape, len(expected), empty_text=empty_text, limit=limit)
                    if json.loads(payload) != obj['answer']:
                        raise RuntimeError('first answer framing drift')
                    chain = work / (path.name.replace('answer-', 'chain-', 1)
                                    .replace('.jsonl', f'-{index}.jsonl'))
                    actual_chains.add(chain.name)
                    pages, page_map, raw_hash, raw_bytes = [], [], hashlib.sha256(), 0
                    with chain.open('rb') as chain_stream:
                        for page_index, raw in enumerate(chain_stream):
                            if not raw.endswith(b'\n'):
                                raise RuntimeError('chain page framing lacks final newline')
                            page_bytes = raw[:-1]
                            if page_index == 0:
                                check_first_page(payload, page_bytes)
                            if page_index >= len(expected) // limit + 2:
                                raise RuntimeError('page-count guard exceeded')
                            raw_hash.update(raw)
                            raw_bytes += len(raw)
                            pages.append(json.loads(page_bytes))
                            page_map.append({'page_index': page_index,
                                'object': retain_bytes(page_bytes, catalog, '.answer.json')})
                            check_space(work.parent, out.parent)
                    if not pages:
                        raise RuntimeError('empty page chain')
                    verdict = grader.query_oracle.check(records, obj['query'], pages)
                    verdicts.append({'file': path.name, 'index': index, 'chain': chain.name,
                                     'verdict': verdict})
                    if not verdict['passed']:
                        dump(out / 'oracle-failure.json', verdicts[-1])
                        raise RuntimeError(f'complete chain oracle failed: {verdict}')
                    if pages[0]['rows'] and 'changed_query_rejected' not in controls:
                        for name in ('changed', 'missing', 'duplicate'):
                            bad = list(pages)
                            bad[0] = copy.deepcopy(pages[0])
                            if name == 'changed':
                                bad[0]['rows'][0]['body'] += 'changed'
                            elif name == 'missing':
                                bad[0]['rows'] = bad[0]['rows'][1:]
                            else:
                                bad[0]['rows'].append(copy.deepcopy(bad[0]['rows'][0]))
                            control = grader.query_oracle.check(records, obj['query'], bad)
                            if control['passed']:
                                raise RuntimeError(f'oracle accepted {name} negative control')
                            controls[name + '_query_rejected'] = control
                    if len(pages) > 1 and 'first_page_only_rejected' not in controls:
                        bad_chains = {'first_page_only': pages[:1], 'truncated_chain': pages[:-1]}
                        for name in ('changed_continuation', 'missing_continuation', 'duplicate_continuation'):
                            bad = list(pages)
                            bad[1] = copy.deepcopy(pages[1])
                            if name == 'changed_continuation':
                                bad[1]['rows'][0]['body'] += 'changed'
                            elif name == 'missing_continuation':
                                bad[1]['rows'] = bad[1]['rows'][1:]
                            else:
                                bad[1]['rows'].append(copy.deepcopy(bad[1]['rows'][0]))
                            bad_chains[name] = bad
                        bad = list(pages)
                        bad[1] = dict(pages[1], snapshot='changed-snapshot')
                        bad_chains['changed_snapshot'] = bad
                        for name, bad in bad_chains.items():
                            control = grader.query_oracle.check(records, obj['query'], bad)
                            if control['passed']:
                                raise RuntimeError(f'oracle accepted {name} negative control')
                            controls[name + '_rejected'] = control
                    mappings.append({'file': path.name, 'index': index,
                        'raw_sha256': hashlib.sha256(line).hexdigest(), 'raw_bytes': len(line),
                        'prefix_hex': prefix.hex(), 'suffix_hex': suffix.hex(),
                        'population': obj['population'], 'iteration': obj['iteration'],
                        'query': obj['query'], 'answer_sha256': hashlib.sha256(payload).hexdigest(),
                        'object': retain_bytes(payload, catalog, '.answer.json')})
                    chain_maps.append({'file': chain.name, 'measured_file': path.name,
                        'measured_index': index, 'pages': page_map,
                        'raw_sha256': raw_hash.hexdigest(), 'raw_bytes': raw_bytes,
                        'format': 'decompress each page object in order, append one LF per page'})
            if calls_seen != 4:
                raise RuntimeError('fewer than four measured calls per shape')
    if actual_chains != {p.name for p in work.glob('chain-*.jsonl')}:
        raise RuntimeError('unassociated chain files')
    if len(verdicts) != 64 or len(controls) != (9 if population == 'full' else 3):
        raise RuntimeError('oracle verdict/negative-control coverage drift')
    dump(out / 'answer-map.json', mappings)
    dump(out / 'chain-map.json', chain_maps)
    return verdicts, controls


def verify_maps(work, out):
    """Compare every reconstructed native wrapper/page against its original bytes."""
    catalog = out.parent / 'objects'
    answer_maps = json.loads((out / 'answer-map.json').read_text())
    chain_maps = json.loads((out / 'chain-map.json').read_text())
    checked_wrappers, checked_pages = 0, 0
    for mapping in answer_maps:
        with gzip.open(catalog / mapping['object'], 'rb') as stream:
            payload = stream.read()
        raw = bytes.fromhex(mapping['prefix_hex']) + payload + bytes.fromhex(mapping['suffix_hex'])
        with (work / mapping['file']).open('rb') as stream:
            for _ in range(mapping['index'] + 1):
                original = stream.readline()
        if raw != original or len(raw) != mapping['raw_bytes'] or hashlib.sha256(raw).hexdigest() != mapping['raw_sha256']:
            raise RuntimeError('lossless measured-wrapper reconstruction failed')
        checked_wrappers += 1
    for mapping in chain_maps:
        digest, size = hashlib.sha256(), 0
        with (work / mapping['file']).open('rb') as original:
            for page_index, page in enumerate(mapping['pages']):
                if page['page_index'] != page_index:
                    raise RuntimeError('chain page order drift')
                with gzip.open(catalog / page['object'], 'rb') as stream:
                    raw = stream.read() + b'\n'
                if raw != original.readline():
                    raise RuntimeError('lossless native page reconstruction failed')
                digest.update(raw)
                size += len(raw)
                checked_pages += 1
            if original.read(1) or size != mapping['raw_bytes'] or digest.hexdigest() != mapping['raw_sha256']:
                raise RuntimeError('lossless chain reconstruction incomplete')
    if checked_wrappers != 64 or len(chain_maps) != 64:
        raise RuntimeError('archive mapping coverage drift')
    return {'wrappers': checked_wrappers, 'chains': len(chain_maps),
            'pages': checked_pages, 'exact_original_bytes_compared': True}


def collect(work, out, variant, population, deadline, *, empty_text=False, limit=10000):
    def expired(signum, frame):
        raise TimeoutError('profiling deadline exhausted during grading/archive verification')
    previous = signal.signal(signal.SIGALRM, expired)
    signal.setitimer(signal.ITIMER_REAL, max(.001, deadline - time.monotonic()))
    try:
        return collect_impl(work, out, variant, population, deadline, empty_text=empty_text, limit=limit)
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous)


def collect_impl(work, out, variant, population, deadline, *, empty_text=False, limit=10000):
    rows = [json.loads(line) for line in (work / 'timings.jsonl').read_text().splitlines()]
    if rows[0]['stage'] != 'fixture' or rows[-1]['stage'] != 'complete':
        raise RuntimeError('probe fixture/completion ledger missing')
    fixture = rows[0]
    if fixture['allocator_counted'] != (variant == 'counted'):
        raise RuntimeError('allocator feature drift')
    source = (work / 'source.log').read_bytes()
    if hashlib.sha256(source).hexdigest() != fixture['source_sha256']:
        raise RuntimeError('source bytes disagree with native fixture receipt')
    expected = source.decode().splitlines()
    records, bodies = grader.decode_records(work / 'records.jsonl')
    grader.exact(expected, bodies)
    measured = [row for row in rows if row['stage'] == 'measured_queries_complete']
    if len(measured) != 1 or measured[0]['continuations_after_all_measurements'] is not True:
        raise RuntimeError('missing measured/continuation boundary')
    # Complete transcripts include the actual measured first pages, without
    # changing the independent oracle or timing their continuation collection.
    began = time.monotonic()
    verdicts, controls = grade_chains(work, out, expected, population, empty_text=empty_text, limit=limit)
    dump(out / 'archive-verification.json', verify_maps(work, out))
    if time.monotonic() >= deadline:
        raise RuntimeError('profiling deadline exhausted during independent grading')
    if len(verdicts) != 64:
        raise RuntimeError('expected 2 kinds * 2 plans * 4 shapes * 4 calls = 64 answers')
    catalog = out.parent / 'objects'
    ledgers, raw_ledgers = {}, {}
    for kind in ('fixture', 'tail', 'segment'):
        path = work / ('records.jsonl' if kind == 'fixture' else kind + '-records/records.jsonl')
        raw_ledgers[kind] = path.read_bytes()
        ledgers[kind] = retain_ledger(raw_ledgers[kind], catalog)
    # Segment is built directly from the fixture; tail uses Intake, which stamps
    # receipt time. Both plans use the same recovered ledger within each layout.
    if raw_ledgers['fixture'] != raw_ledgers['segment']:
        raise RuntimeError('fixture/segment exact record ledgers differ; evidence preserved')
    if not same_batch_records(raw_ledgers['fixture'], raw_ledgers['tail']):
        raise RuntimeError('fixture/recovered records differ beyond received_ns; evidence preserved')
    grader.compress(work / 'timings.jsonl', out / 'timings.jsonl.gz')
    dump(out / 'oracle.json', {'verdicts': verdicts, 'controls': controls,
        'ledger_controls': ledger_controls(),
        'grading_seconds': time.monotonic() - began})
    dump(out / 'fixture.json', {'fixture': fixture, 'records_objects': ledgers,
        'source_bytes': len(source), 'record_count': len(expected),
        'variant': variant, 'population': population, 'measured_complete': measured[0],
        'complete': rows[-1]})
    check_space(work.parent, out.parent)
    return fixture


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path)
    parser.add_argument('--controls', action='store_true')
    parser.add_argument('--build-seconds', type=int, default=900)
    parser.add_argument('--run-seconds', type=int, default=1800)
    args = parser.parse_args()
    if not 0 < args.build_seconds <= 900 or not 0 < args.run_seconds <= 1800:
        parser.error('scope caps: build <=900s; profiling including grading <=1800s')
    require_limits()
    if args.controls:
        print(json.dumps(harness_controls()))
        return
    if args.out is None:
        parser.error('--out required for profiling')
    tmp = Path(os.environ['TMPDIR']).resolve()
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    if not tmp.is_relative_to(STORAGE / 'scratch') or not scratch.is_relative_to(STORAGE / 'scratch'):
        raise RuntimeError('launcher-owned data-drive scratch required')
    out = args.out.resolve()
    if not out.is_relative_to(ROOT / 'docs/experiments/benchmarks/data') or out.exists():
        raise RuntimeError('fresh repository experiment evidence directory required')
    work = scratch / 'completion-query-allocation-profile'
    if work.exists():
        raise RuntimeError('owned profiling scratch already exists')
    out.mkdir(parents=True)
    work.mkdir()
    (work / 'owned').write_text(str(out))
    (work / 'bin').mkdir()
    (out / 'objects').mkdir()
    status = 'interrupted'
    started = time.monotonic()
    commands = []
    try:
        check_space(work, out)
        dump(out / 'harness-controls.json', harness_controls())
        build_deadline = started + args.build_seconds
        for variant in ('plain', 'counted'):
            argv = ['cargo', 'build', '--offline', '--locked', '--release', '-p',
                    'fabric-server', '--example', 'completion_query_probe']
            if variant == 'counted':
                argv += ['--features', 'responsibility-alloc-probe,phase-probe']
            code = run_child(argv, dict(os.environ), out / (variant + '-build.out'),
                             out / (variant + '-build.err'), build_deadline, work, out)
            commands.append({'argv': argv, 'exit': code})
            if code:
                raise RuntimeError(f'{variant} build exit {code}')
            shutil.copy2(Path(os.environ['CARGO_TARGET_DIR']) / 'release/examples/completion_query_probe',
                         work / 'bin' / variant)
        dump(out / 'environment.json', {'command': sys.argv,
            'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
            'binaries': {v: grader.sha(work / 'bin' / v) for v in ('plain', 'counted')},
            'harness_sha256': grader.sha(Path(__file__)), 'grader_sha256': grader.sha(Path(grader.__file__)),
            'native_source_sha256': grader.sha(ROOT / 'crates/fabric-server/examples/completion_query_probe.rs'),
            'oracle_sha256': grader.sha(Path(grader.query_oracle.__file__)),
            'storage': str(work), 'uname': list(os.uname()),
            'cpu_affinity': sorted(os.sched_getaffinity(0)),
            'build_seconds': time.monotonic() - started,
            'limits': {'scratch_bytes': SCRATCH_LIMIT, 'evidence_bytes': EVIDENCE_LIMIT,
                       'free_reserve_bytes': FREE_RESERVE},
            'build_settings': {k: os.environ.get(k) for k in ('RUSTFLAGS', 'CARGO_TARGET_DIR', 'CARGO_BUILD_JOBS')},
            'interpretation': 'single allocation screen; first resets History only; OS page cache warm; fixed Scan then Walk, tail then Segment order; first-page measurements finish before any continuation work; counted timings include combined allocator and phase observer cost; measured phases and continuation phases separated; nested global allocator snapshots overlap and must not be summed as exclusive ownership; measured HWM differs from final HWM including correctness drains; no metrics or rates'})
        deadline = time.monotonic() + args.run_seconds
        full_fixture = None
        for population, count in (('preflight', 128), ('full', 65536)):
            for variant in ('plain', 'counted'):
                label = population + '-' + variant
                trial, evidence = work / label, out / label
                trial.mkdir()
                evidence.mkdir()
                env = dict(os.environ, BENCH_RECORDS=str(count), BENCH_BODY_SIZE='1024',
                    BENCH_ORDER='shuffled', BENCH_QUERY_REPEATS='3', BENCH_OBSERVER='minimal',
                    BENCH_PREFLIGHT='0', BENCH_QUERY_ROTATION='0')
                env['BENCH_PHASES'] = '1' if variant == 'counted' else '0'
                argv = [str(work / 'bin' / variant), str(trial), 'query', '1024']
                code = run_child(argv, env, trial / 'timings.jsonl', evidence / 'probe.err',
                                 deadline, work, out)
                commands.append({'argv': argv, 'exit': code,
                    'bench_env': {k: v for k, v in env.items() if k.startswith('BENCH_')}})
                if code:
                    raise RuntimeError(f'{label} probe exit {code}')
                fixture = collect(trial, evidence, variant, population, deadline)
                phases = trial / 'phases.jsonl'
                if variant == 'counted':
                    if not phases.exists():
                        raise RuntimeError('counted phase-probe output missing')
                    grader.compress(phases, evidence / 'phases.jsonl.gz')
                    grader.compress(trial / 'continuation-phases.jsonl', evidence / 'continuation-phases.jsonl.gz')
                    dump(evidence / 'phase-checks.json', phase_checks(trial))
                elif phases.exists():
                    raise RuntimeError('plain executable unexpectedly includes phase-probe')
                if fixture['records'] != count or fixture['query_warm_repeats'] != 3:
                    raise RuntimeError('fixture/repetition drift')
                if population == 'full':
                    identity = {k: fixture[k] for k in ('source_sha256', 'timestamp_ranks_sha256', 'encoded_batch_bytes')}
                    if full_fixture is not None and identity != full_fixture:
                        raise RuntimeError('plain/counted full fixture drift')
                    full_fixture = identity
                shutil.rmtree(trial)
        status = 'passed'
    except BaseException as error:
        status = 'interrupted' if isinstance(error, KeyboardInterrupt) else 'failed'
        dump(out / 'failure.json', {'status': status, 'error': repr(error), 'scratch_preserved': str(work)})
        raise
    finally:
        dump(out / 'commands.json', commands)
        size = grader.footprint(work)
        if status == 'passed':
            if (work / 'owned').read_text() != str(out):
                raise RuntimeError('scratch ownership mismatch')
            shutil.rmtree(work)
        dump(out / 'cleanup.json', {'status': status, 'work': str(work),
            'removed': not work.exists(), 'scratch_logical_bytes': size,
            'retained_logical_bytes': grader.footprint(out),
            'elapsed_seconds': time.monotonic() - started,
            'failed_scratch_moves_with_launcher_tmpdir': status != 'passed'})
    print(json.dumps({'status': status, 'evidence': str(out)}))


if __name__ == '__main__':
    main()
