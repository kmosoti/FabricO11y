"""CR3 real-query metadata screen; execute registered slices under containment."""
import argparse
import gzip
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits, STORAGE
spec = importlib.util.spec_from_file_location('completion_profile', ROOT / 'tools/bench/labs/completion/profile.py')
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)
import borrowed_log as capacity_accounting

profile.EVIDENCE_LIMIT = 768 * 1024**2

EXAMPLE = 'catalog_many_segment_probe'
_original_space_check = profile.check_space

def campaign_space_check(work, out):
    _original_space_check(work, out)
    capacity_accounting.evidence_inventory()

profile.check_space = campaign_space_check


def dump(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def sha(path):
    return profile.grader.sha(path)


def shape_metrics(row):
    if row['calls'] != 32 or len(row['latency_ns']) != 32 or row['cpu_ns'] <= 0:
        raise RuntimeError('fixed work or CPU observation drift')
    allocation = row['allocation']
    return {'cpu_ns': row['cpu_ns'], 'wall_ns': row['wall_ns'],
            'p95_ns': sorted(row['latency_ns'])[math.ceil(32 * .95) - 1],
            'requested_bytes': allocation['total'] if allocation else None,
            'allocation_calls': allocation['calls'] if allocation else None,
            'incremental_peak_bytes': allocation['peak'] - allocation['base'] if allocation else None}


def guards(pair):
    result = {}
    for shape in ('absent', 'broad'):
        base, candidate = pair['clone'][shape], pair['shared'][shape]
        if pair['counted']:
            if base['requested_bytes'] is None or candidate['requested_bytes'] is None:
                raise RuntimeError('counted observation absent')
            result[shape + ':peak'] = candidate['incremental_peak_bytes'] <= base['incremental_peak_bytes'] * 1.05
            if pair['segments'] == 64 and shape == 'absent':
                result[shape + ':primary'] = candidate['requested_bytes'] <= base['requested_bytes'] * .90
        else:
            for metric in ('cpu_ns', 'wall_ns', 'p95_ns'):
                result[shape + ':' + metric] = candidate[metric] <= base[metric] * 1.05
    return result


def grade(work, evidence, catalog, native, args):
    fixture = native['fixture']
    expected = {'rows': 65536, 'body_bytes': 1024, 'seed': 42, 'segments': args.segments,
                'readers': args.readers, 'variant': evidence.name, 'borrowed_logs': False,
                'plan': 'walk', 'limit': 1000}
    if fixture != expected or [r['shape'] for r in native['results']] != ['absent', 'broad']:
        raise RuntimeError('fixture/shape acknowledgement drift')
    if any((r['allocation'] is not None) != (args.build == 'counted') for r in native['results']):
        raise RuntimeError('plain/count acknowledgement differs')
    records, bodies = profile.grader.decode_records(work / 'records.jsonl')
    if len(bodies) != 65536 or any(len(b) != 1024 for b in bodies):
        raise RuntimeError('raw Batch fixture drift')
    ledger = profile.retain_ledger((work / 'records.jsonl').read_bytes(), catalog)
    maps, verdicts = {}, {}
    for row in native['results']:
        shape = row['shape']
        q = {'kind': 'logs', 'from_ns': 1600000000000000000,
             'to_ns': 1600000000000065536, 'limit': 1000}
        if shape == 'absent':
            q['contains'] = '__CR3_ABSENT__'
        if row['query'] != q:
            raise RuntimeError('declared query differs')
        pages, refs = [], []
        with (work / f'chain-{shape}.jsonl').open('rb') as stream:
            for raw in stream:
                if not raw.endswith(b'\n') or len(pages) >= 68:
                    raise RuntimeError('chain framing/count guard')
                pages.append(json.loads(raw))
                refs.append(profile.retain_bytes(raw[:-1], catalog, '.answer.json'))
        if len(pages) != (1 if shape == 'absent' else 66) or len(pages) != row['pages']:
            raise RuntimeError('full-chain page count differs')
        verdict = profile.grader.query_oracle.check(records, q, pages)
        if shape == 'absent' and (verdict['expected_rows'] != 0 or verdict['answered_rows'] != 0):
            raise RuntimeError('registered absent shape matched rows')
        if not verdict['passed']:
            raise RuntimeError('independent full-chain oracle rejected: ' + repr(verdict))
        with gzip.open(catalog / refs[0], 'rb') as stream:
            first = stream.read()
        calls = []
        with (work / f'calls-{shape}.jsonl').open('rb') as stream:
            for raw in stream:
                if raw != first + b'\n':
                    raise RuntimeError('actual measured first page differs from graded complete chain')
                calls.append({'first_page_object': refs[0], 'raw_sha256': hashlib.sha256(raw).hexdigest()})
        if len(calls) != 32:
            raise RuntimeError('measured query count differs')
        if shape == 'broad':
            # The unchanged oracle must reject an incomplete drain and duplicate row.
            bad = json.loads(json.dumps(pages)); bad[0]['rows'].append(bad[0]['rows'][0])
            if profile.grader.query_oracle.check(records, q, pages[:-1])['passed'] or profile.grader.query_oracle.check(records, q, bad)['passed']:
                raise RuntimeError('full-chain oracle accepted negative control')
        verdicts[shape] = verdict
        maps[shape] = {'query': q, 'canonical_pages': refs, 'measured_calls': calls,
                       'association': 'exact measured first-page bytes and immutable identical query/snapshot; one complete canonical drain'}
    dump(evidence / 'oracle.json', {'verdicts': verdicts, 'truncated_and_duplicate_rejected': True})
    dump(evidence / 'maps.json', {'records_index_object': ledger, 'shapes': maps})
    return sha(work / 'records.jsonl')


def expected_cells():
    return {(segments, readers, counted, rep) for segments in (1, 64)
            for readers in (1, 4) for counted in (False, True) for rep in (1, 2, 3)}


def check_cells(cells):
    if len(cells) != 24 or set(cells) != expected_cells():
        raise RuntimeError('missing/duplicate registered cell')


def check_pair_receipt(obj, frozen):
    pair = obj['pair']
    key = (pair['segments'], pair['readers'], pair['counted'], pair['repetition'])
    if key not in expected_cells() or type(pair['counted']) is not bool:
        raise RuntimeError('unexpected pair cell')
    build = 'counted' if pair['counted'] else 'plain'
    expected_order = ['shared', 'clone'] if pair['repetition'] == 2 else ['clone', 'shared']
    if obj['binary_sha256'] != frozen['binary_sha256'][build] or obj['order'] != expected_order or pair['guards'] != guards(pair):
        raise RuntimeError('pair binary/order/guard receipt differs')
    return key


def check_association(chain, shape, first):
    calls = chain['measured_calls']
    expected = {'first_page_object': chain['canonical_pages'][0],
                'raw_sha256': hashlib.sha256(first + b'\n').hexdigest()}
    query = {'kind': 'logs', 'from_ns': 1600000000000000000,
             'to_ns': 1600000000000065536, 'limit': 1000}
    if shape == 'absent':
        query['contains'] = '__CR3_ABSENT__'
    if chain['query'] != query or len(chain['canonical_pages']) != (1 if shape == 'absent' else 66) or len(calls) != 32 or any(call != expected for call in calls):
        raise RuntimeError('measured association/full drain incomplete or altered')
    return len(calls)


def aggregate_controls():
    import copy
    cells = sorted(expected_cells()); check_cells(cells)
    value = {'cpu_ns': 100, 'wall_ns': 100, 'p95_ns': 100,
             'requested_bytes': None, 'allocation_calls': None, 'incremental_peak_bytes': None}
    pair = {'segments': 64, 'readers': 1, 'counted': False, 'repetition': 1,
            'clone': {'absent': value, 'broad': value}, 'shared': {'absent': value, 'broad': value}}
    pair['guards'] = guards(pair)
    obj = {'pair': pair, 'order': ['clone', 'shared'], 'binary_sha256': 'original'}
    frozen = {'binary_sha256': {'plain': 'original'}}
    check_pair_receipt(obj, frozen)
    chain = {'query': {'kind': 'logs', 'from_ns': 1600000000000000000,
             'to_ns': 1600000000000065536, 'contains': '__CR3_ABSENT__', 'limit': 1000},
             'canonical_pages': ['first'], 'measured_calls': [
                 {'first_page_object': 'first', 'raw_sha256': hashlib.sha256(b'first\n').hexdigest()} for _ in range(32)]}
    check_association(chain, 'absent', b'first')
    changed_guard = copy.deepcopy(obj); changed_guard['pair']['guards']['absent:cpu_ns'] = False
    changed_binary = copy.deepcopy(obj); changed_binary['binary_sha256'] = 'changed'
    changed_chain = copy.deepcopy(chain); changed_chain['measured_calls'][0]['first_page_object'] = 'different'
    broad = copy.deepcopy(chain); broad['query'].pop('contains'); broad['canonical_pages'] *= 66
    check_association(broad, 'broad', b'first')
    null_broad = copy.deepcopy(broad); null_broad['query']['contains'] = None
    rejected = {}
    for name, fn in [('missing_cell', lambda: check_cells(cells[:-1])),
                     ('duplicate_cell', lambda: check_cells(cells[:-1] + [cells[0]])),
                     ('changed_guard', lambda: check_pair_receipt(changed_guard, frozen)),
                     ('changed_binary', lambda: check_pair_receipt(changed_binary, frozen)),
                     ('changed_association', lambda: check_association(changed_chain, 'absent', b'first')),
                     ('broad_contains_null', lambda: check_association(null_broad, 'broad', b'first'))]:
        try:
            fn()
        except RuntimeError:
            rejected[name] = True
        else:
            raise RuntimeError('aggregate validator accepted ' + name)
    return {'unchanged_accepted': True, 'rejected': rejected}


def aggregate(args):
    if args.freeze is None or args.slices is None:
        raise RuntimeError('aggregate requires frozen archive and all24 registered pair slices')
    frozen = json.loads((args.freeze / 'provenance.json').read_text())
    cells, identity, pairs = set(), None, []
    chain_count = call_count = 0
    controls = aggregate_controls()
    for folder in args.slices:
        if folder.parent.resolve() != args.freeze.parent.resolve():
            raise RuntimeError('slice outside run root')
        obj = json.loads((folder / 'result.json').read_text())
        pair = obj['pair']
        key = check_pair_receipt(obj, frozen)
        if key in cells:
            raise RuntimeError('duplicate pair')
        cells.add(key)
        if identity is not None and identity != obj['records_sha256']:
            raise RuntimeError('cross-cell exact Batch ledger differs')
        identity = obj['records_sha256']
        if json.loads((folder / 'cleanup.json').read_text()).get('removed') is not True:
            raise RuntimeError('pair cleanup incomplete')
        expected_order = ['shared', 'clone'] if pair['repetition'] == 2 else ['clone', 'shared']
        if obj['order'] != expected_order or pair['guards'] != guards(pair):
            raise RuntimeError('pair order/guard summary differs')
        for variant in ('clone', 'shared'):
            evidence = folder / variant
            native = json.loads((evidence / 'timings.json').read_text())
            derived = {r['shape']: shape_metrics(r) for r in native['results']}
            if derived != pair[variant] or json.loads((evidence / 'command.json').read_text())['exit'] != 0:
                raise RuntimeError('original metrics/command differs')
            oracle = json.loads((evidence / 'oracle.json').read_text())
            if set(oracle['verdicts']) != {'absent', 'broad'} or any(v.get('passed') is not True for v in oracle['verdicts'].values()):
                raise RuntimeError('complete canonical chain oracle missing/failed')
            maps = json.loads((evidence / 'maps.json').read_text())
            if set(maps['shapes']) != {'absent', 'broad'}:
                raise RuntimeError('query association shape set differs')
            for shape, chain in maps['shapes'].items():
                first_object = chain['canonical_pages'][0]
                with gzip.open(args.freeze.parent / 'objects' / first_object, 'rb') as stream:
                    first = stream.read()
                if first_object != hashlib.sha256(first).hexdigest() + '.answer.json.gz':
                    raise RuntimeError('retained first-page object hash differs')
                call_count += check_association(chain, shape, first)
                chain_count += 1
        pairs.append(pair)
    check_cells(list(cells))
    profile.check_space(args.freeze.parent, args.freeze.parent)
    args.out.mkdir(parents=True, exist_ok=False)
    dump(args.out / 'result.json', {'pairs': pairs, 'native_children': len(pairs) * 2,
         'graded_canonical_complete_chains': chain_count, 'exact_associated_timed_first_pages': call_count, 'aggregate_controls': controls,
         'nomination_complete': True, 'nomination_guards_passed': all(all(p['guards'].values()) for p in pairs),
         'scope': 'fixed H64 real Walk queries; metadata sharing; no service qualification'})


def main():
    require_limits()
    p = argparse.ArgumentParser()
    p.add_argument('--stage', choices=('freeze', 'pair', 'aggregate'), required=True)
    p.add_argument('--out', type=Path, required=True)
    p.add_argument('--freeze', type=Path)
    p.add_argument('--segments', type=int, choices=(1, 64))
    p.add_argument('--readers', type=int, choices=(1, 4))
    p.add_argument('--build', choices=('plain', 'counted'))
    p.add_argument('--pair', type=int, choices=(1, 2, 3))
    p.add_argument('--slices', type=Path, nargs=24)
    args = p.parse_args()
    if args.stage == 'pair' and any(getattr(args, k) is None for k in ('freeze', 'segments', 'readers', 'build', 'pair')):
        p.error('pair requires --freeze --segments --readers --build --pair')
    if args.stage == 'aggregate':
        aggregate(args)
        return
    args.out.mkdir(parents=True, exist_ok=False)
    base = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True)
    if not base.is_relative_to(STORAGE / 'scratch'):
        raise RuntimeError('owned data-drive scratch required')
    work = base / ('many-segment-' + args.out.name)
    work.mkdir()
    dump(args.out / 'admission-inventory.json', capacity_accounting.evidence_inventory())
    env = dict(os.environ)
    for flag in ('FABRIC_BORROWED_LOG_EXPERIMENT', 'FABRIC_SPILL_WORKSPACE_EXPERIMENT', 'BENCH_SHARED_CATALOG'):
        env.pop(flag, None)
    env['FABRIC_BORROWED_LOG_EXPERIMENT'] = '0'
    deadline = time.monotonic() + 540
    binaries = {}
    try:
        if args.stage == 'freeze':
            (args.out / 'binaries').mkdir()
            for name in ('plain', 'counted'):
                argv = ['cargo', 'build', '--offline', '--locked', '--release', '-p', 'fabric-server', '--example', EXAMPLE]
                if name == 'counted':
                    argv += ['--features', 'responsibility-alloc-probe']
                code = profile.run_child(argv, env, args.out / (name + '.out'), args.out / (name + '.err'), deadline, work, args.out.parent)
                dump(args.out / (name + '-command.json'), {'argv': argv, 'exit': code, 'borrowed_compile_flag': '0'})
                if code:
                    raise RuntimeError('example build failed')
                binary = Path(os.environ['CARGO_TARGET_DIR']) / 'release/examples' / EXAMPLE
                binaries[name] = sha(binary)
                with binary.open('rb') as source, gzip.open(args.out / 'binaries' / (name + '.gz'), 'wb') as target:
                    shutil.copyfileobj(source, target)
            sources = [Path(__file__), ROOT / 'crates/fabric-server/examples' / (EXAMPLE + '.rs'),
                       ROOT / 'crates/fabric-server/src/query.rs', ROOT / 'crates/fabric-server/src/read_catalog.rs',
                       ROOT / 'tools/qualification/query_oracle.py', ROOT / 'tools/bench/labs/completion/profile.py',
                       ROOT / 'docs/experiments/benchmarks/catalog-many-segment-protocol.md',
                       ROOT / 'docs/experiments/benchmarks/catalog-many-segment-query-correction-protocol.md',
                       ROOT / 'docs/experiments/benchmarks/fixtures/catalog-many-segment-contains-null.json',
                       ROOT / 'docs/experiments/benchmarks/catalog-evidence-allocation-protocol.md',
                       ROOT / 'tools/bench/labs/catalog/borrowed_log.py']
            dump(args.out / 'provenance.json', {'binary_sha256': binaries,
                 'source_sha256': {str(s.relative_to(ROOT)): sha(s) for s in sources},
                 'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()})
        else:
            if args.out.parent.resolve() != args.freeze.parent.resolve():
                raise RuntimeError('slices must share frozen run root')
            frozen = json.loads((args.freeze / 'provenance.json').read_text())
            binary = work / 'probe'
            with gzip.open(args.freeze / 'binaries' / (args.build + '.gz'), 'rb') as source, binary.open('wb') as target:
                shutil.copyfileobj(source, target)
            binary.chmod(0o700)
            if sha(binary) != frozen['binary_sha256'][args.build]:
                raise RuntimeError('frozen binary changed')
            for source in (Path(__file__), ROOT / 'tools/qualification/query_oracle.py', ROOT / 'tools/bench/labs/completion/profile.py', ROOT / 'tools/bench/labs/catalog/borrowed_log.py'):
                if sha(source) != frozen['source_sha256'][str(source.relative_to(ROOT))]:
                    raise RuntimeError('driver/verifier changed after freeze')
            catalog = args.out.parent / 'objects'; catalog.mkdir(exist_ok=True)
            pair = {'segments': args.segments, 'readers': args.readers,
                    'counted': args.build == 'counted', 'repetition': args.pair}
            identity = None
            order = ('shared', 'clone') if args.pair == 2 else ('clone', 'shared')
            for variant in order:
                evidence = args.out / variant; evidence.mkdir()
                trial = work / variant
                argv = [str(binary), str(trial), str(args.segments), str(args.readers), variant]
                code = profile.run_child(argv, env, evidence / 'timings.json', evidence / 'probe.err', deadline, work, args.out.parent)
                dump(evidence / 'command.json', {'argv': argv, 'exit': code, 'binary_sha256': sha(binary)})
                if code:
                    raise RuntimeError('native query failed')
                native = json.loads((evidence / 'timings.json').read_text())
                current = grade(trial, evidence, catalog, native, args)
                if identity is not None and identity != current:
                    raise RuntimeError('matched raw Batch ledger differs')
                identity = current
                pair[variant] = {r['shape']: shape_metrics(r) for r in native['results']}
                if any((r['allocation'] is not None) != pair['counted'] for r in native['results']):
                    raise RuntimeError('plain/count acknowledgement differs')
                shutil.rmtree(trial)
                profile.check_space(work, args.out.parent)
            pair['guards'] = guards(pair)
            dump(args.out / 'result.json', {'pair': pair, 'order': order, 'records_sha256': identity,
                 'binary_sha256': sha(binary), 'freeze': str(args.freeze), 'nomination_complete': False})
        shutil.rmtree(work)
        dump(args.out / 'cleanup.json', {'removed': True, 'evidence_inventory': capacity_accounting.evidence_inventory()})
    except BaseException as error:
        dump(args.out / 'failure.json', {'error': repr(error), 'retained_scratch': str(work)})
        raise


if __name__ == '__main__':
    main()
