#!/usr/bin/env python3
"""Private, bounded allocation screen using the unchanged native probe and oracle."""
import argparse
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

EVIDENCE_LIMIT = 50 * 1024**2
SCRATCH_LIMIT = 4 * 1024**3


def dump(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def check_space(work, out):
    if grader.footprint(work) > SCRATCH_LIMIT:
        raise RuntimeError('owned profiling scratch exceeds 4GiB')
    evidence_root = next((p for p in (out, *out.parents) if p.name == 'memory'), out)
    if grader.footprint(evidence_root) > EVIDENCE_LIMIT:
        raise RuntimeError('profiling evidence exceeds 50MiB; preserve scratch')
    if shutil.disk_usage(STORAGE).free < SCRATCH_LIMIT:
        raise RuntimeError('4GiB data-drive free-space reserve unavailable')


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


def collect(work, out, variant, population, deadline):
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
    # The unchanged independent grader checks every actual answer and exercises
    # changed/missing/duplicate row controls in each trial, outside native spans.
    began = time.monotonic()
    def expired(signum, frame):
        raise TimeoutError('profiling deadline exhausted during independent grading')
    previous = signal.signal(signal.SIGALRM, expired)
    signal.setitimer(signal.ITIMER_REAL, max(.001, deadline - time.monotonic()))
    try:
        verdicts, controls = grader.grade_query(work, out, expected, False, True)
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous)
    if time.monotonic() >= deadline:
        raise RuntimeError('profiling deadline exhausted during independent grading')
    if len(verdicts) != 64:
        raise RuntimeError('expected 2 kinds * 2 plans * 4 shapes * 4 calls = 64 answers')
    catalog = out.parent / 'objects'
    mappings = []
    for path in sorted(work.glob('answer-*.jsonl')):
        with path.open('rb') as stream:
            for index, line in enumerate(stream):
                # Preserve serde_json's actual answer bytes, not a lossy digest
                # or reconstructed projection. Native wrapper delimiters are fixed.
                payload = line.split(b',"answer":', 1)[1].rsplit(b',"population":', 1)[0]
                obj = json.loads(line)
                if json.loads(payload) != obj['answer']:
                    raise RuntimeError('native answer framing drift')
                mappings.append({'file': path.name, 'index': index,
                    'population': obj['population'], 'iteration': obj['iteration'],
                    'query': obj['query'], 'answer_sha256': hashlib.sha256(payload).hexdigest(),
                    'object': retain_bytes(payload, catalog, '.answer.json')})
        check_space(work.parent, out.parent)
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
    dump(out / 'answer-map.json', mappings)
    dump(out / 'fixture.json', {'fixture': fixture, 'records_objects': ledgers,
        'source_bytes': len(source), 'record_count': len(expected),
        'variant': variant, 'population': population, 'complete': rows[-1]})
    check_space(work.parent, out.parent)
    return fixture


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--build-seconds', type=int, default=300)
    parser.add_argument('--run-seconds', type=int, default=600)
    args = parser.parse_args()
    if not 0 < args.build_seconds <= 300 or not 0 < args.run_seconds <= 600:
        parser.error('scope caps: build <=300s; profiling including grading <=600s')
    require_limits()
    tmp = Path(os.environ['TMPDIR']).resolve()
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    if not tmp.is_relative_to(STORAGE / 'scratch') or not scratch.is_relative_to(STORAGE / 'scratch'):
        raise RuntimeError('launcher-owned data-drive scratch required')
    out = args.out.resolve()
    if not out.is_relative_to(ROOT / 'docs/experiments/benchmarks/data') or out.exists():
        raise RuntimeError('fresh repository experiment evidence directory required')
    work = tmp / 'query-allocation-profile'
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
        build_deadline = started + args.build_seconds
        for variant in ('plain', 'counted'):
            argv = ['cargo', 'build', '--offline', '--locked', '--release', '-p',
                    'fabric-server', '--example', 'responsibility_probe']
            if variant == 'counted':
                argv += ['--features', 'responsibility-alloc-probe,phase-probe']
            code = run_child(argv, dict(os.environ), out / (variant + '-build.out'),
                             out / (variant + '-build.err'), build_deadline, work, out)
            commands.append({'argv': argv, 'exit': code})
            if code:
                raise RuntimeError(f'{variant} build exit {code}')
            shutil.copy2(Path(os.environ['CARGO_TARGET_DIR']) / 'release/examples/responsibility_probe',
                         work / 'bin' / variant)
        dump(out / 'environment.json', {'command': sys.argv,
            'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
            'binaries': {v: grader.sha(work / 'bin' / v) for v in ('plain', 'counted')},
            'harness_sha256': grader.sha(Path(__file__)), 'grader_sha256': grader.sha(Path(grader.__file__)),
            'oracle_sha256': grader.sha(Path(grader.query_oracle.__file__)),
            'storage': str(work), 'uname': list(os.uname()),
            'cpu_affinity': sorted(os.sched_getaffinity(0)),
            'build_seconds': time.monotonic() - started,
            'limits': {'scratch_bytes': SCRATCH_LIMIT, 'evidence_bytes': EVIDENCE_LIMIT},
            'build_settings': {k: os.environ.get(k) for k in ('RUSTFLAGS', 'CARGO_TARGET_DIR', 'CARGO_BUILD_JOBS')},
            'interpretation': 'single allocation screen; first resets History only; OS page cache warm; fixed Scan then Walk, tail then Segment order; counted timings include combined allocator and phase observer cost; nested global allocator snapshots overlap and must not be summed as exclusive ownership; no metrics or rates'})
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
