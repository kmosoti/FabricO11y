#!/usr/bin/env python3
"""CQ2 bounded lazy/eager schedule pairs, retaining every exact oracle chain."""
import argparse
import copy
import gzip
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import STORAGE, require_limits

spec = importlib.util.spec_from_file_location('profile', ROOT / 'tools/bench/labs/completion/profile.py')
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)
compact_spec = importlib.util.spec_from_file_location('compaction', Path(__file__).with_name('compact_evidence.py'))
compaction = importlib.util.module_from_spec(compact_spec)
compact_spec.loader.exec_module(compaction)
START = 1_600_000_000_000_000_000


def usage(directories):
    paths, inodes, logical = set(), {}, 0
    for folder in directories:
        for path in folder.rglob('*'):
            if path.is_file() and not path.is_symlink() and path not in paths:
                paths.add(path)
                stat = path.stat()
                logical += stat.st_size
                inodes[(stat.st_dev, stat.st_ino)] = (stat.st_size, stat.st_blocks * 512)
    sizes = sum(v[0] for v in inodes.values())
    allocated = sum(v[1] for v in inodes.values())
    return {'logical_path_bytes': logical, 'unique_inode_bytes': sizes,
            'allocated_bytes': allocated, 'cap_accounted_bytes': max(sizes, allocated)}


def dump(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def retain_json(raw, catalog):
    """Factor only byte-identical row payload chunks; reconstruct the entire JSON."""
    text = raw.decode('utf-8')
    start = text.index('"rows":[') + len('"rows":')
    _, end = json.JSONDecoder().raw_decode(text, start)
    left = len(text[:start].encode('utf-8'))
    right = len(text[:end].encode('utf-8'))
    payload = raw[left:right]
    objects = [profile.retain_bytes(payload[i:i + 65536], catalog, '.rows.chunk')
               for i in range(0, len(payload), 65536)]
    mapping = {'prefix_hex': raw[:left].hex(), 'chunks': objects,
               'suffix_hex': raw[right:].hex(), 'raw_bytes': len(raw),
               'raw_sha256': hashlib.sha256(raw).hexdigest()}
    verify_json_map(mapping, catalog, raw)
    return mapping


def verify_json_map(mapping, catalog, expected):
    rebuilt = bytearray(bytes.fromhex(mapping['prefix_hex']))
    for name in mapping['chunks']:
        with gzip.open(catalog / name, 'rb') as stream:
            rebuilt.extend(stream.read())
    rebuilt.extend(bytes.fromhex(mapping['suffix_hex']))
    if (bytes(rebuilt) != expected or len(rebuilt) != mapping['raw_bytes'] or
            hashlib.sha256(rebuilt).hexdigest() != mapping['raw_sha256']):
        raise RuntimeError('lossless JSON reconstruction differs from native bytes')


def trial(binary, parent, out, label, eager, rate, seed, skip, deadline):
    began = time.monotonic()
    work = parent / label
    target = out / label
    target.mkdir()
    env = dict(os.environ, BENCH_SEED_RECORDS=str(seed), BENCH_SKIP_REFRESH_AT=str(skip),
               BENCH_PHASES='0', BENCH_SHARED_CATALOG='0', BENCH_EMPTY_TEXT='0',
               BENCH_QUERY_LIMIT='10000', BENCH_ORDER='sorted', BENCH_OBSERVER='minimal')
    argv = [str(binary), str(work), 'eager' if eager else 'lazy', str(rate)]
    commands = [argv]
    environment = {key: env[key] for key in sorted(env) if key.startswith('BENCH_')}
    environment.update({key: env[key] for key in ('FABRIC_SCRATCH_ROOT', 'CARGO_TARGET_DIR') if key in env})
    dump(target / 'commands.json', {'commands': commands, 'environment': environment,
                                    'native_exit_status': 'pending'})
    rc = profile.run_child(argv, env, target / 'stdout.jsonl', target / 'stderr.txt', deadline, parent, out)
    if rc:
        dump(target / 'commands.json', {'commands': commands, 'environment': environment,
                                        'native_exit_status': rc})
        raise RuntimeError(f'{label} native exit {rc}')
    dump(target / 'commands.json', {'commands': commands, 'environment': environment,
                                    'native_exit_status': 0})
    receipt = json.loads((target / 'stdout.jsonl').read_text())
    expected_refreshes = 32 - int(skip == 32) if eager else 0
    for key, expected in {'complete': True, 'eager': eager, 'seed': 42,
                          'seed_records': seed, 'seed_segments': 64 if seed == 65536 else 1, 'append_records': 4096,
                          'query_rate': rate, 'refreshes': expected_refreshes,
                          'skip_refresh_at': skip, 'borrowed_logs': False,
                          'shared_catalog': False, 'answers': 32 * rate + 2}.items():
        if receipt.get(key) != expected:
            raise RuntimeError(f'{label} acknowledgement drift: {key}')
    records, bodies = profile.grader.decode_records(work / 'records.jsonl')
    expected_bodies = (work / 'source.log').read_text().splitlines()
    profile.grader.exact(expected_bodies, bodies)
    if len(records) != seed // 128 + 32:
        raise RuntimeError('one Batch per committed Group fixture changed')
    catalog = out / 'objects'
    ledger = profile.retain_ledger((work / 'records.jsonl').read_bytes(), catalog)
    dump(target / 'ledger-map.json', ledger)
    shutil.copy2(work / 'timings.jsonl', target / 'timings.jsonl')
    rows = [json.loads(line) for line in (work / 'timings.jsonl').read_text().splitlines()]
    total = [row for row in rows if row['stage'] == 'total_schedule']
    if len(total) != 1 or total[0]['queries'] != 32 * rate or total[0]['refreshes'] != expected_refreshes:
        raise RuntimeError('schedule counts changed')
    maps, controls = [], {}
    for i in range(32 * rate + 2):
        wrapper_bytes = (work / f'answer-{i}.json').read_bytes()
        obj = json.loads(wrapper_bytes)
        wrapper_text = wrapper_bytes.decode('utf-8')
        first_start = wrapper_text.index('"answer":') + len('"answer":')
        _, first_end = json.JSONDecoder().raw_decode(wrapper_text, first_start)
        first_bytes = wrapper_text[first_start:first_end].encode('utf-8')
        quiet = i >= 32 * rate
        append = 32 if quiet else i // rate + 1
        newest = seed // 128 + append
        shape = 'broad' if (i - 32 * rate if quiet else i) % 2 == 0 else 'selective'
        query = {'kind': 'logs', 'from_ns': START, 'to_ns': START + seed + 4096, 'limit': 10000}
        if shape == 'selective':
            query['contains'] = 'bench-0007 '
        if obj['id'] != i or obj['query'] != query or obj['newest'] != newest or obj['records'] != newest * 128:
            raise RuntimeError('independent append/query association changed')
        if obj['answer']['snapshot'] != f'g1-{newest}':
            raise RuntimeError('snapshot differs from the actual committed prefix')
        argv = [str(binary), str(work), 'drain', str(i)]
        commands.append(argv)
        dump(target / 'commands.json', {'commands': commands, 'environment': environment,
                                        'native_exit_status': 0, 'completed_drains': len(maps),
                                        'pending_drain': i})
        rc = profile.run_child(argv, env, target / 'drain.stdout', target / 'drain.stderr', deadline, parent, out)
        if rc:
            raise RuntimeError(f'{label} drain {i} exit {rc}')
        chain = work / 'chain.jsonl'
        pages, objects = [], []
        for page_index, raw in enumerate(chain.read_bytes().splitlines()):
            if page_index == 0 and raw != first_bytes:
                raise RuntimeError('measured first-page bytes changed during drain')
            page = json.loads(raw)
            pages.append(page)
            objects.append(retain_json(raw, catalog))
        if not pages or pages[0] != obj['answer']:
            raise RuntimeError('measured first-page association changed')
        prefix = records[:newest]
        verdict = profile.grader.query_oracle.check(prefix, query, pages)
        if not verdict['passed']:
            dump(target / 'oracle-failure.json', {'id': i, 'verdict': verdict})
            raise RuntimeError('unchanged full-chain oracle failed')
        if pages[0]['rows'] and not controls:
            for control in ('changed', 'missing', 'duplicate'):
                bad = copy.deepcopy(pages)
                if control == 'changed':
                    bad[0]['rows'][0]['body'] += 'MUTATION'
                elif control == 'missing':
                    bad[0]['rows'].pop(0)
                else:
                    bad[0]['rows'].append(copy.deepcopy(bad[0]['rows'][0]))
                result = profile.grader.query_oracle.check(prefix, query, bad)
                if result['passed']:
                    raise RuntimeError(f'oracle accepted {control}')
                controls[control] = result
        wrapper_object = retain_json(wrapper_bytes, catalog)
        if 'archive_changed_framing' not in controls:
            for control in ('changed_framing', 'missing_chunk', 'duplicate_chunk'):
                bad = copy.deepcopy(wrapper_object)
                if control == 'changed_framing':
                    bad['prefix_hex'] += '00'
                elif control == 'missing_chunk':
                    bad['chunks'].pop()
                else:
                    bad['chunks'].append(bad['chunks'][0])
                try:
                    verify_json_map(bad, catalog, wrapper_bytes)
                except RuntimeError:
                    controls['archive_' + control] = {'rejected': True}
                else:
                    raise RuntimeError('archive checker accepted ' + control)
        maps.append({'id': i, 'prefix_batches': newest, 'wrapper': wrapper_object,
                     'pages': objects, 'verdict': verdict, 'quiet': quiet})
        dump(target / 'answer-map.json', maps)
        dump(target / 'controls.json', controls)
        dump(target / 'commands.json', {'commands': commands, 'environment': environment,
                                        'native_exit_status': 0, 'completed_drains': len(maps),
                                        'all_completed_drain_exit_status': 0})
        # retain_json already re-opened every chunk and compared whole native bytes.
        chain.unlink()
        profile.check_space(parent, out)
        if time.monotonic() >= deadline:
            raise RuntimeError('grading deadline exhausted')
    dump(target / 'answer-map.json', maps)
    dump(target / 'controls.json', controls)
    dump(target / 'commands.json', {'commands': commands, 'all_exit_status': 0,
                                    'environment': environment})
    result = {'label': label, 'eager': eager, 'seed_records': seed, 'rate': rate,
              'skip': skip, 'answers': len(maps), 'total': total[0],
              'binary_sha256': profile.grader.sha(binary), 'control_count': len(controls),
              'native_and_grading_seconds': time.monotonic() - began}
    dump(target / 'result.json', result)
    shutil.rmtree(work)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--freeze', type=Path, required=True)
    parser.add_argument('--objects', type=Path, required=True)
    parser.add_argument('--account', type=Path, nargs='+', required=True)
    parser.add_argument('--cap-mib', type=int, choices=[256, 1024], default=256)
    parser.add_argument('--reserve-mib', type=int, default=96)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--rate', type=int, choices=[0, 1, 4], required=True)
    parser.add_argument('--slice', choices=['preflight', 'diagnostic', 'pair1', 'pair2', 'pair3'], required=True)
    parser.add_argument('--seconds', type=int, default=1200)
    args = parser.parse_args()
    require_limits()
    if not 0 < args.seconds <= 1500:
        parser.error('seconds must be 1..1500')
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    out = args.out.resolve()
    if not scratch.is_relative_to(STORAGE / 'scratch') or not out.is_relative_to(ROOT / 'docs/experiments/benchmarks/data') or out.exists():
        raise RuntimeError('fresh mounted scratch and repository evidence required')
    out.mkdir(parents=True)
    freeze = args.freeze.resolve(strict=True)
    objects = args.objects.resolve()
    accounts = [path.resolve(strict=True) for path in args.account]
    data = ROOT / 'docs/experiments/benchmarks/data'
    if not freeze.is_relative_to(data) or any(not p.is_dir() or not p.is_relative_to(data) for p in accounts):
        raise RuntimeError('repository freeze and full campaign account directories required')
    if objects.parent != data or not objects.name.startswith('catalog-maintenance-objects-'):
        raise RuntimeError('dedicated catalog maintenance object pool required')
    if not objects.exists():
        objects.mkdir()
        (objects / 'ownership.json').write_text('{"owner":"catalog-maintenance-v1"}\n')
    if (objects / 'ownership.json').read_text() != '{"owner":"catalog-maintenance-v1"}\n':
        raise RuntimeError('shared object ownership differs')
    (out / 'objects').symlink_to(objects, target_is_directory=True)
    accounts = list(dict.fromkeys([*accounts, freeze, objects, out]))
    if not 0 <= args.reserve_mib <= 1024:
        parser.error('reserve-mib must be0..1024')
    observed = usage(accounts)
    dump(out / 'admission.json', {'usage': observed, 'accounts': [str(p) for p in accounts],
                                 'cap_bytes': args.cap_mib * 1024**2,
                                 'projected_reserve_bytes': args.reserve_mib * 1024**2})
    if observed['cap_accounted_bytes'] + args.reserve_mib * 1024**2 > args.cap_mib * 1024**2:
        raise RuntimeError('projected persistent query allocation exhausted')
    work = scratch / 'catalog-maintenance'
    work.mkdir()
    deadline = time.monotonic() + args.seconds
    binaries = {}
    frozen_receipt = json.loads((freeze / 'freeze.json').read_text())
    if frozen_receipt['status'] != 'complete' or frozen_receipt['environment']['FABRIC_BORROWED_LOG_EXPERIMENT'] != '0':
        raise RuntimeError('incomplete or wrong-selector frozen build')
    for name in ('tools/bench/labs/catalog/maintenance.py',
                 'tools/bench/labs/catalog/compact_evidence.py',
                 'tools/bench/labs/completion/profile.py', 'tools/qualification/query_oracle.py'):
        if frozen_receipt['source_hashes'].get(name) != profile.grader.sha(ROOT / name):
            raise RuntimeError('frozen driver/verifier changed: ' + name)
    for variant in ('plain', 'counted'):
        path = work / variant
        identity = frozen_receipt['binaries'][variant]
        archive = freeze / identity['archive']
        if profile.grader.sha(archive) != identity['archive_sha256']:
            raise RuntimeError('frozen executable archive differs')
        with gzip.open(archive, 'rb') as src, path.open('xb') as dest:
            shutil.copyfileobj(src, dest)
        path.chmod(0o700)
        if profile.grader.sha(path) != identity['decoded_sha256'] or path.stat().st_size != identity['decoded_bytes']:
            raise RuntimeError('frozen executable decoded bytes differ')
        binaries[variant] = path
    source_paths = ['crates/fabric-server/examples/catalog_maintenance_probe.rs',
                    'crates/fabric-server/src/read_catalog.rs', 'crates/fabric-server/src/query.rs',
                    'crates/fabric-server/src/tail.rs', 'crates/fabric-server/src/segment.rs',
                    'tools/bench/labs/catalog/maintenance.py', 'tools/bench/labs/completion/profile.py',
                    'docs/experiments/benchmarks/catalog-maintenance-protocol.md', 'Cargo.lock']
    dump(out / 'environment.json', {'source_hashes_at_launch': {name: profile.grader.sha(ROOT / name)
                                                               for name in source_paths},
                                   'oracle_sha256': profile.grader.sha(Path(profile.grader.query_oracle.__file__)),
                                   'binary_hashes': {key: profile.grader.sha(path) for key, path in binaries.items()},
                                   'driver_argv': sys.argv, 'counted_compile_feature': 'responsibility-alloc-probe',
                                   'freeze_directory': str(freeze), 'freeze_receipt': frozen_receipt,
                                   'required_compile_selector': 'FABRIC_BORROWED_LOG_EXPERIMENT=0',
                                   'build_source_archive': 'root coordinator frozen-build receipt'})
    rows, status = [], 'interrupted'
    try:
        profile.check_space(work, out)
        if args.slice == 'preflight':
            schedule = [('plain', 128, False, 0), ('plain', 128, True, 0),
                        ('plain', 128, True, 32)]
        elif args.slice == 'diagnostic':
            schedule = [('counted', 65536, False, 0), ('counted', 65536, True, 0)]
        else:
            order = [False, True] if args.slice in ('pair1', 'pair3') else [True, False]
            schedule = [('plain', 65536, eager, 0) for eager in order]
        for index, (variant, seed, eager, skip) in enumerate(schedule):
            if usage(accounts)['cap_accounted_bytes'] > args.cap_mib * 1024**2:
                raise RuntimeError('persistent query allocation exhausted before next trial')
            label = f'{index}-{variant}-{seed}-{"eager" if eager else "lazy"}-skip{skip}'
            result = trial(binaries[variant], work, out, label, eager, args.rate, seed, skip, deadline)
            if json.loads((out / label / 'stdout.jsonl').read_text())['counted'] != (variant == 'counted'):
                raise RuntimeError('allocation variant changed')
            rows.append(result)
            dump(out / 'results.json', rows)
            transformations = []
            def keep(entry):
                if not any(old is entry for old in transformations):
                    transformations.append(entry)
                dump(out / label / 'compaction.json', transformations)
            for path in list((out / label).glob('*.json')):
                if path.stat().st_size >= 1024**2:
                    compaction.compress_json(path, keep)
            observed = usage(accounts)
            dump(out / 'evidence-accounting.json', observed)
            if observed['cap_accounted_bytes'] > args.cap_mib * 1024**2:
                raise RuntimeError('persistent query allocation exceeded; preserve evidence')
        dump(out / 'results.json', rows)
        profile.check_space(work, out)
        status = 'complete'
    finally:
        dump(out / 'cleanup.json', {'status': status, 'scratch': str(work), 'removed': status == 'complete'})
        if status == 'complete':
            shutil.rmtree(work)


if __name__ == '__main__':
    main()
