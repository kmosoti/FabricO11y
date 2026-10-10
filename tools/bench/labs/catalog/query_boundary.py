#!/usr/bin/env python3
"""CQ1 preservation and shared-metadata screen with unchanged full-chain grading."""
import argparse
import gzip
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys
import time
import statistics

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import STORAGE, require_limits

spec = importlib.util.spec_from_file_location('completion_profile', ROOT / 'tools/bench/labs/completion/profile.py')
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)
archive_spec = importlib.util.spec_from_file_location('catalog_archive', Path(__file__).with_name('compact_evidence.py'))
compaction = importlib.util.module_from_spec(archive_spec)
archive_spec.loader.exec_module(compaction)


def campaign_usage(directories):
    entries = {}
    seen_paths = set()
    logical = 0
    for directory in dict.fromkeys(directories):
        for path in directory.rglob('*'):
            if path.is_file() and not path.is_symlink():
                if path in seen_paths:
                    continue
                seen_paths.add(path)
                stat = path.stat()
                logical += stat.st_size
                entries[(stat.st_dev, stat.st_ino)] = (stat.st_size, stat.st_blocks * 512)
    lengths = sum(row[0] for row in entries.values())
    allocated = sum(row[1] for row in entries.values())
    return {'logical_path_bytes': logical, 'unique_inode_bytes': lengths,
            'allocated_bytes': allocated, 'cap_accounted_bytes': max(lengths, allocated)}


def dump(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def extract(archive, target):
    with gzip.open(archive, 'rb') as source, target.open('wb') as output:
        shutil.copyfileobj(source, output)
    target.chmod(0o700)


def metrics(work):
    rows = [json.loads(line) for line in (work / 'timings.jsonl').read_text().splitlines()]
    warm = [r for r in rows if r['stage'] == 'query_tail_walk_broad_warm']
    if len(warm) != 3:
        raise RuntimeError('expected three broad-tail warm measurements')
    boundary = [r for r in rows if r['stage'] == 'measured_queries_complete']
    return {'wall_ns': statistics.median(r['wall_ns'] for r in warm),
            'cpu_ns': statistics.median(r['cpu_ns'] for r in warm),
            'measured_hwm_kib': boundary[0]['vm_hwm_kib'],
            'warm': warm}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--baseline-plain', type=Path, required=True)
    parser.add_argument('--baseline-counted', type=Path, required=True)
    parser.add_argument('--plain', type=Path)
    parser.add_argument('--counted', type=Path)
    parser.add_argument('--slice', choices=['diagnostic', 'pair1', 'pair2', 'pair3'], required=True)
    parser.add_argument('--reuse-build', type=Path)
    parser.add_argument('--account', type=Path, nargs='*', default=[])
    parser.add_argument('--cap-mib', type=int, choices=[256, 1024], default=256)
    parser.add_argument('--reserve-mib', type=int, default=240)
    parser.add_argument('--run-seconds', type=int, default=1200)
    args = parser.parse_args()
    if not 0 < args.run_seconds <= 1500 or ((args.plain is None) != (args.counted is None)):
        parser.error('run<=1500s; supply both current binary paths or neither')
    if args.reuse_build and args.plain:
        parser.error('choose archived build or explicit binaries')
    if args.reuse_build and not args.account:
        parser.error('reused slice requires explicit full campaign --account directories')
    if not 0 <= args.reserve_mib <= 1024:
        parser.error('reserve-mib must be 0..1024')
    require_limits()
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    if not scratch.is_relative_to(STORAGE / 'scratch'):
        raise RuntimeError('launcher-owned mounted-drive scratch required')
    out = args.out.resolve()
    if not out.is_relative_to(ROOT / 'docs/experiments/benchmarks/data') or out.exists():
        raise RuntimeError('fresh repository evidence directory required')
    work = scratch / 'catalog-query-boundary'
    if work.exists():
        raise RuntimeError('owned scratch already exists')
    out.mkdir(parents=True)
    accounts = [path.resolve(strict=True) for path in args.account]
    if any(not path.is_dir() or not path.is_relative_to(ROOT / 'docs/experiments/benchmarks/data') for path in accounts):
        raise RuntimeError('accounting directories must be repository evidence')
    if args.reuse_build:
        objects = (args.reuse_build / 'objects').resolve(strict=True)
        if not objects.is_dir() or not objects.is_relative_to(ROOT / 'docs/experiments/benchmarks/data'):
            raise RuntimeError('owned shared objects directory required')
        (out / 'objects').symlink_to(objects, target_is_directory=True)
    else:
        (out / 'objects').mkdir()
        objects = out / 'objects'
    accounts = list(dict.fromkeys([*accounts, out, objects]))
    usage = campaign_usage(accounts)
    dump(out / 'admission.json', {'observed': usage, 'accounts': [str(p) for p in accounts],
                                 'cap_bytes': args.cap_mib * 1024**2,
                                 'reserved_next_slice_bytes': args.reserve_mib * 1024**2})
    if usage['cap_accounted_bytes'] + args.reserve_mib * 1024**2 > args.cap_mib * 1024**2:
        raise RuntimeError('projected campaign evidence exceeds explicit allocation')
    work.mkdir()
    (work / 'owned').write_text(str(out))
    (work / 'bin').mkdir()
    commands, rows = [], []
    status = 'interrupted'
    began = time.monotonic()
    try:
        profile.check_space(work, out)
        for variant in ('plain', 'counted'):
            archive = getattr(args, 'baseline_' + variant).resolve()
            decoded = work / 'bin' / ('baseline-' + variant)
            extract(archive, decoded)
            expected = {'plain': '7500edaeae90f7e4bf00875af0f1f5f059d096a8184b936551b4a7d4e44b672c',
                        'counted': '321ec3ac1048c1b2a8f63bb8baef68e9c6347eb7b0a69e16251a9c535637acf3'}
            if profile.grader.sha(decoded) != expected[variant]:
                raise RuntimeError('pre-boundary binary provenance mismatch')
        build_deadline = time.monotonic() + 300
        origin = None
        if args.reuse_build:
            origin = json.loads((args.reuse_build / 'environment.json').read_text())
        for variant in ('plain', 'counted'):
            if origin is not None:
                decoded = work / 'bin' / ('current-' + variant)
                extract(args.reuse_build / 'binaries' / ('current-' + variant + '.gz'), decoded)
                if profile.grader.sha(decoded) != origin['binary_hashes']['current-' + variant]:
                    raise RuntimeError('reused candidate binary differs from frozen build')
                continue
            frozen = getattr(args, variant)
            if frozen is None:
                argv = ['cargo', 'build', '--offline', '--locked', '--release', '-p', 'fabric-server',
                        '--example', 'completion_query_probe']
                if variant == 'counted':
                    argv += ['--features', 'responsibility-alloc-probe,phase-probe']
                build_env = dict(os.environ, FABRIC_BORROWED_LOG_EXPERIMENT='0')
                build_env.pop('FABRIC_SPILL_WORKSPACE_EXPERIMENT', None)
                code = profile.run_child(argv, build_env, out / f'{variant}-build.out',
                                         out / f'{variant}-build.err', build_deadline, work, out)
                commands.append({'argv': argv, 'exit': code})
                if code:
                    raise RuntimeError(f'build {variant} exit {code}')
                frozen = Path(os.environ['CARGO_TARGET_DIR']) / 'release/examples/completion_query_probe'
            shutil.copy2(frozen, work / 'bin' / ('current-' + variant))
        dump(out / 'environment.json', {
            'argv': sys.argv, 'cpu_affinity': sorted(os.sched_getaffinity(0)), 'uname': list(os.uname()),
            'storage': str(work), 'baseline_archives': {
                v: {'path': str(getattr(args, 'baseline_' + v)), 'sha256': profile.grader.sha(getattr(args, 'baseline_' + v))}
                for v in ('plain', 'counted')},
            'binary_hashes': {p.name: profile.grader.sha(p) for p in (work / 'bin').iterdir()},
            'source_hashes': {name: profile.grader.sha(ROOT / name) for name in (
                'crates/fabric-server/src/query.rs', 'crates/fabric-server/src/read_catalog.rs',
                'crates/fabric-server/src/tail.rs', 'crates/fabric-server/examples/completion_query_probe.rs',
                'tools/qualification/query_oracle.py', 'tools/bench/labs/completion/profile.py')},
            'harness_sha256': profile.grader.sha(Path(__file__)),
            'archive_helper_sha256': profile.grader.sha(Path(__file__).with_name('compact_evidence.py')),
            'shared_objects': str(objects), 'campaign_accounts': [str(p) for p in accounts],
            'frozen_candidate_origin': None if origin is None else {'directory': str(args.reuse_build), 'environment': origin},
            'interpretation': 'pre-boundary baseline vs corrected catalog clone vs shared immutable metadata; plain timings primary; warm OS cache; no CQ2 policy change'})
        (out / 'binaries').mkdir()
        for variant in ('plain', 'counted'):
            name = 'current-' + variant
            destination = out / 'binaries' / (name + '.gz')
            if args.reuse_build:
                retained = args.reuse_build / 'binaries' / (name + '.gz')
                compaction.equal_payload(work / 'bin' / name, retained, False, True)
                os.link(retained, destination)
            else:
                with (work / 'bin' / name).open('rb') as source, gzip.open(destination, 'wb') as dest:
                    shutil.copyfileobj(source, dest)
        deadline = time.monotonic() + args.run_seconds
        controls = profile.harness_controls()
        dump(out / 'harness-controls.json', controls)
        schedule = []
        pairs = [] if args.slice == 'diagnostic' else [int(args.slice[-1])]
        if args.slice == 'diagnostic':
            schedule.extend(('preflight', 128, v, 0, m) for v in ('plain', 'counted')
                            for m in ('baseline', 'clone', 'shared'))
            schedule.extend(('full', 65536, 'counted', 1, m) for m in ('baseline', 'clone', 'shared'))
        for pair in pairs:
            order = ('baseline', 'clone', 'shared') if pair % 2 else ('shared', 'clone', 'baseline')
            schedule.extend(('full', 65536, 'plain', pair, m) for m in order)
        identities = {}
        for population, count, variant, pair, mechanism in schedule:
            usage = campaign_usage(accounts)
            if usage['cap_accounted_bytes'] > args.cap_mib * 1024**2:
                raise RuntimeError('campaign evidence allocation exhausted before next trial')
            label = f'{population}-{variant}-p{pair}-{mechanism}'
            trial, evidence = work / label, out / label
            trial.mkdir()
            evidence.mkdir()
            binary = work / 'bin' / (('baseline-' if mechanism == 'baseline' else 'current-') + variant)
            env = dict(os.environ, BENCH_RECORDS=str(count), BENCH_BODY_SIZE='1024',
                       BENCH_ORDER='shuffled', BENCH_QUERY_REPEATS='3', BENCH_OBSERVER='minimal',
                       BENCH_PREFLIGHT='0', BENCH_QUERY_ROTATION='0',
                       BENCH_EMPTY_TEXT='0',
                       BENCH_SHARED_CATALOG='1' if mechanism == 'shared' else '0',
                       BENCH_PHASES='1' if variant == 'counted' else '0')
            # Freeze unsupported exploratory controls to their legacy values too.
            env['BENCH_QUERY_LIMIT'] = '10000'
            env['BENCH_QUERY_SEGMENTS'] = '1'
            env['BENCH_EMPTY_TEXT'] = '0'
            argv = [str(binary), str(trial), 'query', '1024']
            code = profile.run_child(argv, env, trial / 'timings.jsonl', evidence / 'probe.err',
                                     deadline, work, out)
            commands.append({'argv': argv, 'exit': code,
                             'bench_env': {k: v for k, v in env.items() if k.startswith('BENCH_')}})
            if code:
                raise RuntimeError(f'{label} exit {code}')
            observed = metrics(trial)
            fixture = profile.collect(trial, evidence, variant, population, deadline)
            if mechanism != 'baseline' and fixture.get('catalog_shared') != (mechanism == 'shared'):
                raise RuntimeError('current binary did not acknowledge shared metadata selector')
            if mechanism != 'baseline' and fixture.get('borrowed_logs') is not False:
                raise RuntimeError('CQ1 current binary did not disable borrowed Parquet experiment')
            identity = tuple(fixture[k] for k in ('source_sha256', 'timestamp_ranks_sha256', 'encoded_batch_bytes'))
            if count in identities and identity != identities[count]:
                raise RuntimeError('matched fixture drift')
            identities[count] = identity
            if fixture['records'] != count or fixture['query_warm_repeats'] != 3:
                raise RuntimeError('declared count/repetition drift')
            if variant == 'counted':
                dump(evidence / 'phase-checks.json', profile.phase_checks(trial))
                for name in ('phases', 'continuation-phases'):
                    profile.grader.compress(trial / (name + '.jsonl'), evidence / (name + '.jsonl.gz'))
            elif (trial / 'phases.jsonl').exists():
                raise RuntimeError('plain executable contains phase observer')
            transformations = []
            def retain_transform(entry):
                if not any(old is entry for old in transformations):
                    transformations.append(entry)
                dump(evidence / 'compaction.json', transformations)
            for path in list(evidence.glob('*.json')):
                if path.stat().st_size >= 1024**2:
                    compaction.compress_json(path, retain_transform)
            usage = campaign_usage(accounts)
            dump(out / 'evidence-accounting.json', usage)
            if usage['cap_accounted_bytes'] > args.cap_mib * 1024**2:
                raise RuntimeError('campaign evidence allocation exceeded; preserve exact artifacts')
            rows.append({'population': population, 'variant': variant, 'pair': pair,
                         'mechanism': mechanism, 'evidence': label, 'metrics': observed})
            dump(out / 'trials.json', rows)
            shutil.rmtree(trial)
        comparisons = []
        for pair in pairs:
            selected = {r['mechanism']: r['metrics'] for r in rows if r['population'] == 'full'
                        and r['variant'] == 'plain' and r['pair'] == pair}
            for candidate, baseline in (('clone', 'baseline'), ('shared', 'clone')):
                ratios = {k: selected[candidate][k] / selected[baseline][k]
                          for k in ('wall_ns', 'cpu_ns', 'measured_hwm_kib')}
                comparisons.append({'pair': pair, 'baseline': baseline, 'candidate': candidate,
                                    'ratios': ratios, 'cq1_guard_met': all(v <= 1.05 for v in ratios.values())
                                    if candidate == 'clone' else None})
        dump(out / 'comparison.json', comparisons)
        if len(rows) != (9 if args.slice == 'diagnostic' else 3):
            raise RuntimeError('incomplete matrix slice')
        status = 'completed'
    except BaseException as error:
        status = 'interrupted' if isinstance(error, (KeyboardInterrupt, TimeoutError)) else 'failed'
        dump(out / 'failure.json', {'status': status, 'error': repr(error), 'scratch_preserved': str(work)})
        raise
    finally:
        dump(out / 'commands.json', commands)
        scratch_bytes = profile.grader.footprint(work)
        if status == 'completed':
            if (work / 'owned').read_text() != str(out):
                raise RuntimeError('scratch ownership mismatch')
            shutil.rmtree(work)
        dump(out / 'cleanup.json', {'status': status, 'removed': not work.exists(),
             'scratch': str(work), 'scratch_bytes': scratch_bytes, 'evidence_bytes': profile.grader.footprint(out),
             'elapsed_seconds': time.monotonic() - began, 'trials': len(rows)})
    print(json.dumps({'status': status, 'out': str(out), 'trials': len(rows)}))


if __name__ == '__main__':
    main()
