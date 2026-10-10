"""CR2 matched query screen using unchanged complete-pagination oracle adapter."""
import argparse
import gzip
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits, STORAGE
spec = importlib.util.spec_from_file_location('completion_profile', ROOT / 'tools/bench/labs/completion/profile.py')
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)
import compact_evidence as compaction

FLAG = 'FABRIC_BORROWED_LOG_EXPERIMENT'
profile.EVIDENCE_LIMIT = 768 * 1024**2  # registered catalog allocation amendment
DATA = ROOT / 'docs/experiments/benchmarks/data'
CAPACITY_PREFIXES = ('catalog-spill-', 'catalog-builder-spill-', 'catalog-borrowed-',
                     'catalog-many-segment-', 'catalog-lifetime-', 'catalog-baseline-freeze-')
_original_space_check = profile.check_space


def evidence_inventory():
    capacity, catalog, logical, blocks = {}, {}, 0, 0
    roots = list(DATA.glob('catalog-*'))
    for lab in ('coordinator', 'memory', 'query', 'recovery'):
        roots += list((DATA / 'lab-completion-run-01' / lab).glob('catalog-*'))
    for root in roots:
        for path in root.rglob('*'):
            if not path.is_file():
                continue
            st = path.stat()
            key = (st.st_dev, st.st_ino)
            catalog[key] = (st.st_size, st.st_blocks * 512)
            logical += st.st_size
            if root.name.startswith(CAPACITY_PREFIXES):
                capacity[key] = (st.st_size, st.st_blocks * 512)
    result = {'capacity_unique_bytes': sum(v[0] for v in capacity.values()),
              'capacity_allocated_bytes': sum(v[1] for v in capacity.values()),
              'catalog_unique_bytes': sum(v[0] for v in catalog.values()),
              'catalog_allocated_bytes': sum(v[1] for v in catalog.values()),
              'catalog_logical_path_bytes': logical}
    if max(result['capacity_unique_bytes'], result['capacity_allocated_bytes']) > 768 * 1024**2:
        raise RuntimeError('registered aggregate capacity evidence exceeds768MiB')
    if max(result['catalog_unique_bytes'], result['catalog_allocated_bytes']) > 2 * 1024**3:
        raise RuntimeError('registered aggregate catalog evidence exceeds2GiB')
    return result


def campaign_space_check(work, out):
    _original_space_check(work, out)
    evidence_inventory()


profile.check_space = campaign_space_check


def guards(pair):
    base, candidate = pair['baseline'], pair['candidate']
    if set(base) != set(candidate):
        raise RuntimeError('paired query population drift')
    result = {}
    for stage in base:
        if pair['counted']:
            if any(base[stage][k] is None or candidate[stage][k] is None for k in ('requested_bytes', 'incremental_peak_bytes')):
                raise RuntimeError('counted query allocation evidence absent')
            result[stage + ':peak'] = candidate[stage]['incremental_peak_bytes'] <= base[stage]['incremental_peak_bytes'] * 1.05
            if stage in ('query_segment_scan_selective_warm', 'query_segment_walk_selective_warm'):
                result[stage + ':primary'] = candidate[stage]['requested_bytes'] <= base[stage]['requested_bytes'] * .9
        else:
            for metric in ('cpu_ns', 'wall_ns'):
                result[stage + ':' + metric] = candidate[stage][metric] <= base[stage][metric] * 1.05
    return result


def dump(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def metrics(path):
    opener = gzip.open if path.suffix == '.gz' else open
    with opener(path, 'rt') as stream:
        rows = [json.loads(line) for line in stream]
    result = {}
    for row in rows:
        stage = row['stage']
        if stage.startswith('query_') and stage.endswith('_warm'):
            result.setdefault(stage, []).append(row)
    if len(result) != 16 or any(len(v) != 3 for v in result.values()):
        raise RuntimeError('expected16 query populations with3 warm measurements')
    return {stage: {'cpu_ns': statistics.median(r['cpu_ns'] for r in values),
                    'wall_ns': statistics.median(r['wall_ns'] for r in values),
                    'requested_bytes': statistics.median(r['allocation']['cumulative_requested_bytes'] for r in values) if values[0].get('allocation') is not None else None,
                    'incremental_peak_bytes': statistics.median(r['allocation']['incremental_peak_bytes'] for r in values) if values[0].get('allocation') is not None else None}
            for stage, values in result.items()}


def load_json(path):
    if path.exists():
        return json.loads(path.read_text())
    with gzip.open(Path(str(path) + '.gz'), 'rt') as stream:
        return json.load(stream)


def check_verdicts(verdicts):
    expected = {(f'answer-{kind}-{plan}-{shape}.jsonl', i)
                for kind in ('tail', 'segment') for plan in ('scan', 'walk')
                for shape in ('empty', 'selective', 'common', 'broad') for i in range(4)}
    actual = [(v['file'], v['index']) for v in verdicts]
    if len(actual) != 64 or set(actual) != expected or any(v['verdict'].get('passed') is not True for v in verdicts):
        raise RuntimeError('missing/duplicate/failed full-chain verdict')
    for v in verdicts:
        expected_chain = v['file'].replace('answer-', 'chain-', 1).replace('.jsonl', f"-{v['index']}.jsonl")
        if v['chain'] != expected_chain:
            raise RuntimeError('oracle chain association differs')
    return len(actual)


def check_slice_summary(obj, frozen, seen):
    if obj.get('stage') != 'pair' or obj.get('full_chain_oracle_collected') is not True or len(obj['pairs']) != 1:
        raise RuntimeError('incomplete slice')
    pair = obj['pairs'][0]
    if type(pair['counted']) is not bool or pair['repetition'] not in (1, 2, 3):
        raise RuntimeError('unexpected pair population')
    key = (pair['counted'], pair['repetition'])
    if key in seen:
        raise RuntimeError('duplicate slice')
    expected_order = ['candidate', 'baseline'] if pair['repetition'] == 2 else ['baseline', 'candidate']
    if list(pair['order']) != expected_order or set(pair) != {'counted', 'repetition', 'order', 'baseline', 'candidate', 'nomination_guards'}:
        raise RuntimeError('pair variant/order differs')
    expected_names = {('counted' if pair['counted'] else 'plain') + '-' + mechanism for mechanism in ('baseline', 'candidate')}
    if set(obj['binary_sha256']) != expected_names or any(frozen['binary_sha256'].get(name) != digest for name, digest in obj['binary_sha256'].items()):
        raise RuntimeError('slice binary provenance differs')
    if pair['nomination_guards'] != guards(pair):
        raise RuntimeError('slice guard summary differs')
    return key, pair


def aggregation_controls():
    import copy
    verdicts = [{'file': f'answer-{kind}-{plan}-{shape}.jsonl', 'index': i,
                 'chain': f'chain-{kind}-{plan}-{shape}-{i}.jsonl', 'verdict': {'passed': True}}
                for kind in ('tail', 'segment') for plan in ('scan', 'walk')
                for shape in ('empty', 'selective', 'common', 'broad') for i in range(4)]
    check_verdicts(verdicts)
    value = {'cpu_ns': 100, 'wall_ns': 100, 'requested_bytes': None, 'incremental_peak_bytes': None}
    pair = {'counted': False, 'repetition': 1, 'order': ['baseline', 'candidate'],
            'baseline': {'stage': value}, 'candidate': {'stage': value}}
    pair['nomination_guards'] = guards(pair)
    obj = {'stage': 'pair', 'full_chain_oracle_collected': True, 'pairs': [pair],
           'binary_sha256': {'plain-baseline': 'a', 'plain-candidate': 'b'}}
    frozen = {'binary_sha256': obj['binary_sha256']}
    check_slice_summary(obj, frozen, set())
    mutations = [('missing_verdict', lambda: check_verdicts(verdicts[:-1])),
                 ('duplicate_verdict', lambda: check_verdicts(verdicts[:-1] + [verdicts[0]])),
                 ('duplicate_pair', lambda: check_slice_summary(obj, frozen, {(False, 1)}))]
    wrong = copy.deepcopy(obj); wrong['binary_sha256']['plain-baseline'] = 'changed'
    mutations.append(('changed_provenance', lambda: check_slice_summary(wrong, frozen, set())))
    changed = copy.deepcopy(obj); changed['pairs'][0]['nomination_guards']['stage:cpu_ns'] = False
    mutations.append(('changed_guard', lambda: check_slice_summary(changed, frozen, set())))
    rejected = {}
    for name, fn in mutations:
        try:
            fn()
        except RuntimeError:
            rejected[name] = True
        else:
            raise RuntimeError('aggregate validator accepted negative control: ' + name)
    return {'unchanged_accepted': True, 'rejected': rejected}


def check_trial(evidence, pair, mechanism, identity):
    counted = pair['counted']
    receipts = load_json(evidence / 'compaction.json')['entries']
    for filename in ('answer-map.json', 'chain-map.json', 'oracle.json', 'fixture.json'):
        logical = str((evidence / filename).resolve().relative_to(ROOT))
        receipt = receipts.get(logical)
        retained = evidence / (filename + '.gz')
        if receipt is None or receipt.get('exact_readback_compared') is not True or receipt.get('original_removed') is not True:
            raise RuntimeError('trial compaction incomplete')
        if receipt['retained_path'] != str(retained.resolve().relative_to(ROOT)) or (evidence / filename).exists():
            raise RuntimeError('trial compaction path differs')
        if compaction.digest(retained, True) != (receipt['decoded_sha256'], receipt['decoded_bytes']) or compaction.digest(retained) != (receipt['new_compressed_sha256'], receipt['new_compressed_bytes']):
            raise RuntimeError('trial exact archive hashes/lengths differ')
    native = load_json(evidence / 'fixture.json')['fixture']
    expected = {'borrowed_logs': mechanism == 'candidate', 'allocator_counted': counted,
                'catalog_shared': False, 'empty_text': True, 'query_limit': 10000,
                'records': 65536, 'body_bytes': 1024, 'seed': 42, 'query_warm_repeats': 3,
                'timestamp_order': 'shuffled'}
    if any(native.get(k) != v for k, v in expected.items()):
        raise RuntimeError('native trial population differs')
    if [native[k] for k in ('source_sha256', 'timestamp_ranks_sha256', 'encoded_batch_bytes')] != identity:
        raise RuntimeError('trial fixture identity differs from slice')
    if load_json(evidence / 'command.json')['exit'] != 0:
        raise RuntimeError('trial command incomplete')
    archive = load_json(evidence / 'archive-verification.json')
    if archive.get('wrappers') != 64 or archive.get('chains') != 64 or archive.get('exact_original_bytes_compared') is not True:
        raise RuntimeError('exact archive verification incomplete')
    observed = metrics(evidence / 'timings.jsonl.gz')
    if observed != pair[mechanism] or any((v['requested_bytes'] is not None) != counted for v in observed.values()):
        raise RuntimeError('timings-derived metrics differ from pair summary')
    return check_verdicts(load_json(evidence / 'oracle.json')['verdicts'])


ADMISSION_PROTOCOL = ROOT / 'docs/experiments/benchmarks/catalog-evidence-capacity-admission-protocol.md'
DRIVER_KEY = 'tools/bench/labs/catalog/borrowed_log.py'
VERIFIER_KEYS = ('tools/bench/labs/completion/profile.py', 'tools/qualification/query_oracle.py',
                 'tools/bench/labs/catalog/compact_evidence.py')


def validate_execution_admission(manifest, frozen, freeze_sha, driver_sha, verifier_sha, protocol_sha):
    expected = {'version': 1, 'scope': 'resource-only capacity evidence reallocation',
                'freeze_provenance_sha256': freeze_sha,
                'original_driver_sha256': frozen['source_sha256'][DRIVER_KEY],
                'replacement_driver_sha256': driver_sha,
                'binary_sha256': frozen['binary_sha256'],
                'unchanged_verifier_sha256': {key: frozen['source_sha256'][key] for key in VERIFIER_KEYS},
                'capacity_evidence_bytes': 768 * 1024**2, 'aggregate_catalog_bytes': 2 * 1024**3,
                'reviewed_protocol_sha256': protocol_sha}
    if manifest != expected or verifier_sha != expected['unchanged_verifier_sha256']:
        raise RuntimeError('execution admission does not bind the reviewed resource-only replacement')


def execution_identity(args, frozen):
    driver_sha = sha(Path(__file__))
    verifier_sha = {key: sha(ROOT / key) for key in VERIFIER_KEYS}
    if any(verifier_sha[key] != frozen['source_sha256'][key] for key in VERIFIER_KEYS):
        raise RuntimeError('frozen verifier changed')
    if args.execution_admission is None:
        if driver_sha != frozen['source_sha256'][DRIVER_KEY]:
            raise RuntimeError('changed driver requires a bound --execution-admission')
        return None
    manifest = load_json(args.execution_admission)
    freeze_sha = sha(args.freeze / 'provenance.json')
    protocol_sha = sha(ADMISSION_PROTOCOL)
    validate_execution_admission(manifest, frozen, freeze_sha, driver_sha, verifier_sha, protocol_sha)
    rejected = {}
    import copy
    for field in ('freeze_provenance_sha256', 'original_driver_sha256', 'replacement_driver_sha256',
                  'binary_sha256', 'unchanged_verifier_sha256', 'reviewed_protocol_sha256'):
        changed = copy.deepcopy(manifest); changed[field] = 'changed'
        try:
            validate_execution_admission(changed, frozen, freeze_sha, driver_sha, verifier_sha, protocol_sha)
        except RuntimeError:
            rejected[field] = True
        else:
            raise RuntimeError('execution admission validator accepted changed binding: ' + field)
    return {'manifest': manifest, 'manifest_source_sha256': sha(args.execution_admission),
            'negative_bindings_rejected': rejected}


def aggregate(args):
    if args.freeze is None or args.slices is None:
        raise RuntimeError('aggregate requires --freeze and exactly six --slices')
    frozen = json.loads((args.freeze / 'provenance.json').read_text())
    admission = execution_identity(args, frozen)
    if json.loads((args.freeze / 'result.json').read_text()).get('controls_passed') is not True:
        raise RuntimeError('freeze controls are incomplete')
    for name, expected in frozen['binary_sha256'].items():
        digest = hashlib.sha256()
        with gzip.open(args.freeze / 'binaries' / (name + '.gz'), 'rb') as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                digest.update(chunk)
        if digest.hexdigest() != expected:
            raise RuntimeError('frozen archived binary changed: ' + name)
    trials, identity, seen = [], None, set()
    chains = 0
    controls = aggregation_controls()
    for folder in args.slices:
        if folder.parent.resolve() != args.freeze.parent.resolve():
            raise RuntimeError('slice is outside frozen run root')
        provenance = load_json(folder / 'provenance.json')
        if provenance.get('original_provenance') != frozen:
            raise RuntimeError('slice original freeze provenance differs')
        recorded = provenance.get('execution_admission')
        if recorded is None:
            if provenance.get('execution_driver_sha256', frozen['source_sha256'][DRIVER_KEY]) != frozen['source_sha256'][DRIVER_KEY]:
                raise RuntimeError('unamended slice driver differs from original freeze')
        else:
            if admission is None or recorded != admission or provenance.get('execution_driver_sha256') != admission['manifest']['replacement_driver_sha256']:
                raise RuntimeError('slice execution admission differs')
        obj = load_json(folder / 'result.json')
        if provenance['binary_sha256'] != obj['binary_sha256']:
            raise RuntimeError('slice binary result/provenance differs')
        key, pair = check_slice_summary(obj, frozen, seen)
        seen.add(key)
        if load_json(folder / 'cleanup.json').get('removed') is not True:
            raise RuntimeError('slice cleanup incomplete')
        current = obj['fixture_identity']
        if identity is not None and current != identity:
            raise RuntimeError('cross-slice fixture drift')
        identity = current
        build = 'counted' if pair['counted'] else 'plain'
        expected_dirs = {f'{build}-{pair["repetition"]}-{mechanism}' for mechanism in ('baseline', 'candidate')}
        if {p.name for p in folder.iterdir() if p.is_dir() and not p.is_symlink()} != expected_dirs:
            raise RuntimeError('trial variant set differs')
        for mechanism in ('baseline', 'candidate'):
            chains += check_trial(folder / f'{build}-{pair["repetition"]}-{mechanism}', pair, mechanism, current)
        trials.append(pair)
    if seen != {(counted, repetition) for counted in (False, True) for repetition in (1, 2, 3)}:
        raise RuntimeError('six registered pairs are required')
    if profile.grader.footprint(args.freeze.parent) > profile.EVIDENCE_LIMIT:
        raise RuntimeError('owned CR2 evidence exceeds768MiB')
    inventory = evidence_inventory()
    args.out.mkdir(parents=True, exist_ok=False)
    dump(args.out / 'result.json', {'pairs': sorted(trials, key=lambda p: (p['counted'], p['repetition'])),
         'full_chain_oracle_collected': True, 'native_children': len(trials) * 2, 'complete_chains': chains, 'aggregation_controls': controls, 'evidence_inventory': inventory,
         'nomination_guards_passed': all(all(p['nomination_guards'].values()) for p in trials),
         'execution_admission': admission, 'freeze': str(args.freeze), 'slices': [str(p) for p in args.slices],
         'scope': 'unchanged registered CR2 workload/decision rule; independently contained pair slices'})


def main():
    require_limits()
    parser = argparse.ArgumentParser()
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--stage', choices=('freeze', 'pair', 'aggregate'), required=True)
    parser.add_argument('--freeze', type=Path)
    parser.add_argument('--execution-admission', type=Path)
    parser.add_argument('--build', choices=('plain', 'counted'))
    parser.add_argument('--pair', type=int, choices=(1, 2, 3))
    parser.add_argument('--slices', type=Path, nargs=6)
    args = parser.parse_args()
    if args.stage == 'aggregate':
        aggregate(args)
        return
    if args.stage == 'pair' and (args.freeze is None or args.build is None or args.pair is None):
        parser.error('pair requires --freeze, --build and --pair')
    args.out.mkdir(parents=True, exist_ok=False)
    if args.stage == 'freeze':
        (args.out.parent / 'objects').mkdir(exist_ok=True)
    else:
        if args.out.parent.resolve() != args.freeze.parent.resolve():
            raise RuntimeError('all slices must share the frozen run root')
    dump(args.out / 'admission-inventory.json', evidence_inventory())
    (args.out / 'objects').symlink_to((args.out.parent / 'objects').resolve(), target_is_directory=True)
    dump(args.out / 'harness-controls.json', profile.harness_controls())
    stage = 'query_segment_scan_selective_warm'
    baseline = {'cpu_ns': 100, 'wall_ns': 100, 'requested_bytes': 100, 'incremental_peak_bytes': 100}
    candidate = dict(baseline, requested_bytes=85)
    for counted, field, bad in ((True, 'requested_bytes', 91), (True, 'incremental_peak_bytes', 106),
                                (False, 'cpu_ns', 106), (False, 'wall_ns', 106)):
        pair = {'counted': counted, 'baseline': {stage: baseline}, 'candidate': {stage: candidate}}
        if not all(guards(pair).values()):
            raise RuntimeError('unchanged nomination control rejected')
        pair['candidate'] = {stage: dict(candidate, **{field: bad})}
        if all(guards(pair).values()):
            raise RuntimeError('nomination negative control accepted: ' + field)
    dump(args.out / 'nomination-controls.json', {'unchanged_accepted': True, 'representative_regressions_rejected': True})
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True)
    if not scratch.is_relative_to(STORAGE / 'scratch'):
        raise RuntimeError('owned data-drive scratch required')
    work = scratch / ('borrowed-log-' + args.stage + '-' + args.out.name)
    work.mkdir()
    (work / 'bin').mkdir()
    env = dict(os.environ)
    env.pop(FLAG, None)
    env.pop('FABRIC_SPILL_WORKSPACE_EXPERIMENT', None)
    env.pop('BENCH_SHARED_CATALOG', None)
    deadline = time.monotonic() + 1400
    binaries, hashes, trials = {}, {}, []
    try:
        if args.stage == 'freeze':
            opted = dict(env, **{FLAG: '1'})
            controls = ['cargo', 'test', '--offline', '--locked', '-p', 'fabric-server', '--lib', 'borrowed_log_tests']
            code = profile.run_child(controls, opted, args.out / 'controls.out', args.out / 'controls.err', deadline, work, args.out)
            if code:
                raise RuntimeError('borrowed validation controls failed')
            dump(args.out / 'controls-command.json', {'argv': controls, 'exit': code, 'flag': '1'})
            for counted in (False, True):
                for candidate in (False, True):
                    name = ('counted' if counted else 'plain') + ('-candidate' if candidate else '-baseline')
                    build = ['cargo', 'build', '--offline', '--locked', '--release', '-p', 'fabric-server', '--example', 'completion_query_probe']
                    if counted:
                        build += ['--features', 'responsibility-alloc-probe,phase-probe']
                    code = profile.run_child(build, opted if candidate else env, args.out / (name + '-build.out'), args.out / (name + '-build.err'), deadline, work, args.out)
                    dump(args.out / (name + '-build-command.json'), {'argv': build, 'exit': code, 'flag': '1' if candidate else None})
                    if code:
                        raise RuntimeError('build failed: ' + name)
                    frozen = work / 'bin' / name
                    shutil.copy2(Path(os.environ['CARGO_TARGET_DIR']) / 'release/examples/completion_query_probe', frozen)
                    binaries[name], hashes[name] = frozen, sha(frozen)
            dump(args.out / 'provenance.json', {'binary_sha256': hashes, 'source_sha256': {
                 str(path.relative_to(ROOT)): sha(path) for path in (Path(__file__),
                 ROOT / 'crates/fabric-server/src/segment.rs', ROOT / 'crates/fabric-server/src/query.rs',
                 ROOT / 'crates/fabric-server/examples/completion_query_probe.rs',
                 ROOT / 'tools/qualification/query_oracle.py', ROOT / 'tools/bench/labs/completion/profile.py',
             ROOT / 'tools/bench/labs/catalog/compact_evidence.py',
                 ROOT / 'docs/experiments/benchmarks/catalog-borrowed-log-protocol.md',
                 ROOT / 'docs/experiments/benchmarks/catalog-borrowed-log-slicing-protocol.md',
             ROOT / 'docs/experiments/benchmarks/catalog-evidence-allocation-protocol.md')},
                 'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
                 'cache': 'fresh process; warm generated fixture, three warm queries per shape'})
            archive = args.out / 'binaries'
            archive.mkdir()
            for name, binary in binaries.items():
                with binary.open('rb') as source, gzip.open(archive / (name + '.gz'), 'wb') as target:
                    shutil.copyfileobj(source, target)
            dump(args.out / 'result.json', {'stage': 'freeze', 'controls_passed': True,
                 'binary_sha256': hashes, 'performance_measured': False})
            shutil.rmtree(work)
            dump(args.out / 'cleanup.json', {'removed': True})
            return
        frozen = json.loads((args.freeze / 'provenance.json').read_text())
        hashes = frozen['binary_sha256']
        freeze_result = json.loads((args.freeze / 'result.json').read_text())
        if freeze_result.get('stage') != 'freeze' or freeze_result.get('controls_passed') is not True or freeze_result.get('binary_sha256') != hashes:
            raise RuntimeError('freeze controls/provenance incomplete')
        admission = execution_identity(args, frozen)
        for mechanism in ('baseline', 'candidate'):
            name = args.build + '-' + mechanism
            binary = work / 'bin' / name
            with gzip.open(args.freeze / 'binaries' / (name + '.gz'), 'rb') as source, binary.open('wb') as target:
                shutil.copyfileobj(source, target)
            binary.chmod(0o700)
            if sha(binary) != hashes[name]:
                raise RuntimeError('archived frozen binary SHA mismatch: ' + name)
            binaries[name] = binary
        dump(args.out / 'provenance.json', {'freeze': str(args.freeze), 'binary_sha256': {
             name: hashes[name] for name in binaries}, 'original_provenance': frozen,
             'execution_driver_sha256': sha(Path(__file__)), 'execution_admission': admission})
        expected_identity = None
        for counted in (args.build == 'counted',):
            build = 'counted' if counted else 'plain'
            for repetition in (args.pair - 1,):
                order = ('baseline', 'candidate') if repetition % 2 == 0 else ('candidate', 'baseline')
                pair = {}
                for mechanism in order:
                    name = build + '-' + mechanism
                    stem = f'{build}-{repetition+1}-{mechanism}'
                    trial = work / stem
                    trial.mkdir()
                    evidence = args.out / stem
                    evidence.mkdir()
                    native_env = dict(env, BENCH_RECORDS='65536', BENCH_ORDER='shuffled',
                                      BENCH_QUERY_REPEATS='3', BENCH_EMPTY_TEXT='1',
                                      BENCH_OBSERVER='minimal', BENCH_OBSERVER_BATCH='20000',
                                      BENCH_QUERY_ROTATION='0', BENCH_QUERY_LIMIT='10000',
                                      BENCH_BODY_SIZE='1024', BENCH_PREFLIGHT='0',
                                      BENCH_PHASES='1' if counted else '0')
                    argv = [str(binaries[name]), str(trial), 'query', '1024']
                    code = profile.run_child(argv, native_env, trial / 'timings.jsonl', evidence / 'probe.err', deadline, work, args.out)
                    dump(evidence / 'command.json', {'argv': argv, 'exit': code, 'bench_env': {k: v for k, v in native_env.items() if k.startswith('BENCH_')}})
                    if code:
                        raise RuntimeError('native query failed: ' + stem)
                    if profile.grader.footprint(args.out.parent) > profile.EVIDENCE_LIMIT:
                        raise RuntimeError('owned CR2 evidence exceeds768MiB')
                    observed = metrics(trial / 'timings.jsonl')
                    if any((value['requested_bytes'] is not None) != counted for value in observed.values()):
                        raise RuntimeError('plain/count binary allocation acknowledgement differs')
                    fixture = profile.collect(trial, evidence, build, 'full', deadline, empty_text=True, limit=10000)
                    if fixture.get('borrowed_logs') != (mechanism == 'candidate'):
                        raise RuntimeError('wrong borrowed selector')
                    if fixture.get('empty_text') is not True or fixture.get('query_limit') != 10000:
                        raise RuntimeError('native query shape acknowledgement differs')
                    identity = tuple(fixture[k] for k in ('source_sha256', 'timestamp_ranks_sha256', 'encoded_batch_bytes'))
                    if expected_identity is not None and identity != expected_identity:
                        raise RuntimeError('matched fixture drift')
                    expected_identity = identity
                    if counted:
                        dump(evidence / 'phase-checks.json', profile.phase_checks(trial))
                        for phase in ('phases', 'continuation-phases'):
                            profile.grader.compress(trial / (phase + '.jsonl'), evidence / (phase + '.jsonl.gz'))
                    elif (trial / 'phases.jsonl').exists():
                        raise RuntimeError('plain trial unexpectedly contains phase observer')
                    # collect has already compared maps to original measured bytes.
                    # Preserve an incremental exact transformation receipt before unlink.
                    entries = {}
                    def retain_compaction(entry):
                        entries[entry['original_path']] = dict(entry)
                        dump(evidence / 'compaction.json', {'entries': entries})
                    for filename in ('answer-map.json', 'chain-map.json', 'oracle.json', 'fixture.json'):
                        compaction.compress_json(evidence / filename, retain_compaction)
                    pair[mechanism] = observed
                    if profile.grader.footprint(args.out.parent) > profile.EVIDENCE_LIMIT:
                        raise RuntimeError('owned CR2 evidence exceeds768MiB')
                    shutil.rmtree(trial)
                measured = {'counted': counted, 'repetition': repetition+1, 'order': order, **pair}
                measured['nomination_guards'] = guards(measured)
                trials.append(measured)
                dump(args.out / 'pairs.json', trials)
        if any(sha(binary) != hashes[name] for name, binary in binaries.items()):
            raise RuntimeError('frozen binary changed')
        dump(args.out / 'result.json', {'stage': 'pair', 'fixture_identity': expected_identity,
             'binary_sha256': {name: hashes[name] for name in binaries}, 'pairs': trials, 'full_chain_oracle_collected': True,
             'pair_guards_passed': all(all(p['nomination_guards'].values()) for p in trials),
             'nomination_complete': False, 'performance_failure_is_collected_evidence': True,
             'scope': 'H64 borrowed Parquet log projection; measured attributes empty; no service qualification'})
        shutil.rmtree(work)
        dump(args.out / 'cleanup.json', {'removed': True, 'evidence_inventory': evidence_inventory()})
    except BaseException as error:
        dump(args.out / 'failure.json', {'error': repr(error), 'retained_scratch': str(work)})
        raise


if __name__ == '__main__':
    main()
