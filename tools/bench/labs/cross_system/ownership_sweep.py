#!/usr/bin/env python3
"""Opt-in early ownership release; separate frozen build/cell jobs, launcher only."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

import memory_census as mc
import storage_sweep as sweep
from storage_confirmation import screen, controls as cost_controls

REGISTERED = mc.ROOT/'docs/experiments/benchmarks/data/native-frontier-01/memory'
PROPOSAL = mc.ROOT/'docs/experiments/benchmarks/native-ownership-proposal.md'
ENV = 'FABRIC_EARLY_ROW_RELEASE_EXPERIMENT'
ARMS = ('baseline', 'candidate')
SEEDS = {'screen': (2703204357, 2703204358), 'confirmation': (2703204359, 2703204360)}


def ownership_screen(candidate, baseline):
    result = screen(candidate, baseline)
    ratios = result['ratios_8_to_16']
    result['guards']['benefit_at_least_10_percent'] = ratios['heap'] <= .9
    result['guards']['build_write_within_1_percent'] = (
        ratios['write'] is not None and .99 <= ratios['write'] <= 1.01)
    return result


def controls():
    result = cost_controls()
    expected = {'state/segments/0001/manifest.json': {'bytes': 9, 'sha256': 'a'*64}}
    changed = {'state/segments/0001/manifest.json': {'bytes': 9, 'sha256': 'b'*64}}
    try:
        mc.equal_member_maps(expected, changed)
    except ValueError:
        result['mutated_manifest_content_hash_rejected'] = True
    else:
        raise AssertionError('changed exact output content hash accepted')
    def row(heap=100, seconds=1, cpu=1, rss=100, writes=100):
        return {'probe': {'peak_live_heap_bytes': heap, 'encoded_group_bytes': 100,
                'build_seconds_instrumented': seconds,
                'io_before': 'write_bytes: 0\n', 'io_after': f'write_bytes: {writes}\n'},
                'process': {'user_seconds': cpu, 'system_seconds': 0,
                            'whole_process_peak_rss_kib': rss}}
    baseline = row()
    if not all(ownership_screen(row(heap=80), baseline)['guards'].values()):
        raise AssertionError('representative ownership benefit rejected')
    for name, defect in [('no_heap_gain', row()), ('throughput_loss', row(heap=80, seconds=2)),
                         ('cpu_regression', row(heap=80, cpu=2)), ('rss_regression', row(heap=80, rss=200)),
                         ('write_increase', row(heap=80, writes=102)),
                         ('write_decrease', row(heap=80, writes=98))]:
        if all(ownership_screen(defect, baseline)['guards'].values()):
            raise AssertionError('ownership screen accepted defect: '+name)
        result['ownership_'+name+'_rejected'] = True
    zero = ownership_screen(row(heap=80, writes=0), row(writes=0))
    if zero['guards']['build_write_within_1_percent']:
        raise AssertionError('unknown zero-write IO guard accepted')
    result['ownership_unknown_zero_write_rejected'] = True
    return result


def identity(protocol):
    paths = [mc.PROBE, mc.ROOT/'Cargo.lock', PROPOSAL, protocol, Path(__file__),
             Path(__file__).with_name('ownership_with_decoder.py'),
             Path(mc.__file__), Path(sweep.__file__),
             Path(__file__).with_name('storage_confirmation.py')]
    for name in ('crates/fabric-server/src', 'crates/fabric-frame/src'):
        paths += sorted((mc.ROOT/name).rglob('*.rs'))
    return {str(path.resolve().relative_to(mc.ROOT)): mc.digest(path) for path in paths}


def build(args):
    binaries = args.binaries_root.resolve()
    if binaries.exists() or not binaries.is_relative_to(mc.DATA.resolve()):
        raise ValueError('fresh owned data-drive binary root required')
    binaries.mkdir(parents=True)
    before = identity(args.protocol)
    records = {}
    for arm in ARMS:
        env = dict(os.environ, CARGO_BUILD_JOBS='2')
        cleared = sorted(name for name in env if name.startswith('FABRIC_') and name.endswith('_EXPERIMENT'))
        for name in (*cleared, *sweep.EXPERIMENT_ENV, ENV, 'FABRIC_KEY_FIRST_EXPERIMENT',
                     'FABRIC_QUERY_KEY_FIRST_EXPERIMENT'):
            env.pop(name, None)
        env['FABRIC_RUN_MIB_EXPERIMENT'] = '8'
        if arm == 'candidate':
            env[ENV] = '1'
        command = ['cargo', 'build', '--offline', '--locked', '--release', '-p', 'fabric-server',
                   '--example', 'readiness_memory_lab']
        started = time.monotonic()
        with (args.destination/f'build-{arm}.log').open('wb') as log:
            result = subprocess.run(command, cwd=mc.ROOT, env=env, stdout=log,
                                    stderr=subprocess.STDOUT, timeout=360)
        record = {'argv': command, 'exit_code': result.returncode,
                  'elapsed_seconds': time.monotonic()-started, 'cargo_build_jobs': 2,
                  'compile_env': {name: env.get(name) for name in (*sweep.EXPERIMENT_ENV, ENV)},
                  'rustflags': env.get('RUSTFLAGS')}
        record['cleared_experiment_environment'] = cleared
        record['key_first_environment'] = {name: env.get(name) for name in
            ('FABRIC_KEY_FIRST_EXPERIMENT', 'FABRIC_QUERY_KEY_FIRST_EXPERIMENT')}
        mc.dump(args.destination/f'build-{arm}.json', record)
        result.check_returncode()
        binary = binaries/arm
        shutil.copyfile(Path(os.environ['CARGO_TARGET_DIR'])/'release/examples/readiness_memory_lab', binary)
        binary.chmod(0o755)
        records[arm] = {'path': str(binary), 'sha256': mc.digest(binary), 'build': record}
    if identity(args.protocol) != before:
        raise ValueError('source/protocol changed during builds')
    mc.dump(args.destination/'build-manifest.json', {'binaries': records, 'source_sha256': before,
                'protocol_sha256': mc.digest(args.protocol), 'proposal_sha256': mc.digest(PROPOSAL)})
    mc.dump(args.destination/'complete.json', {'exit_code': 0, 'build_count': 2,
                'retained_binary_cache': str(binaries), 'qualification': False})


def run(args):
    manifest = json.loads(args.build_manifest.read_text())
    if manifest['source_sha256'] != identity(args.protocol):
        raise ValueError('frozen source/protocol differs')
    binaries = {}
    for arm in ARMS:
        record = manifest['binaries'][arm]
        binary = Path(record['path']).resolve()
        settings = record['build']['compile_env']
        if not binary.is_relative_to(mc.DATA.resolve()) or mc.digest(binary) != record['sha256']:
            raise ValueError('frozen binary differs')
        if settings['FABRIC_RUN_MIB_EXPERIMENT'] != '8' or settings[ENV] != ('1' if arm == 'candidate' else None):
            raise ValueError('ownership/run-size compile setting mismatch')
        binaries[arm] = binary
    if args.stage == 'confirmation':
        if args.screen_receipts is None or len(args.screen_receipts) != 2:
            raise ValueError('confirmation requires both prospective screen receipts')
        for repeat, path in enumerate(args.screen_receipts):
            value = json.loads(path.read_text())
            if not (value['stage'] == 'screen' and value['seed'] == SEEDS['screen'][repeat]
                    and value['all_screens_passed'] and value['all_semantic_gates']
                    and value['build_manifest_sha256'] == mc.digest(args.build_manifest)):
                raise ValueError('conditional screen admission failed')
    seed = SEEDS[args.stage][args.repeat]
    deadline = time.monotonic()+840
    mc.dump(args.destination/'negative-controls.json', controls())
    mc.dump(args.destination/'metadata.json', {'stage': args.stage, 'seed': seed, 'target_mib': 64,
        'build_manifest_sha256': mc.digest(args.build_manifest), **manifest,
        **mc.decoder_call('version', deadline), 'qualification': False,
        'ledger_provenance': 'unchanged Rust differential; independent Python physical FTF1',
        'screen_admission_receipt_sha256': {str(p): mc.digest(p) for p in (args.screen_receipts or [])}})
    work = Path(tempfile.mkdtemp(prefix='ownership-', dir=os.environ['FABRIC_SCRATCH_ROOT']))
    pairs = []
    try:
        for index, shape in enumerate(sweep.SHAPES):
            sweep.guard(work, args.destination, deadline, reserve=True)
            pair_dir = args.destination/shape
            pair_dir.mkdir()
            order = list(ARMS) if (index+args.repeat) % 2 == 0 else list(reversed(ARMS))
            rows, states, maps = {}, [], {}
            for arm in order:
                cell = pair_dir/arm
                cell.mkdir()
                state = work/f'{shape}-{arm}'
                states.append(state)
                rows[arm] = sweep.native(binaries[arm], state, cell, shape, 64, seed,
                    'bounded', mc.cgroup(), work, args.destination, deadline)
                maps[arm] = json.loads((cell/'complete-member-map.json').read_text())
            gates = mc.grade(rows['baseline']['probe'], rows['candidate']['probe'])
            # Independent content reads of every regular journal/output member.
            mc.equal_member_maps(maps['baseline'], maps['candidate'])
            gates['all_regular_member_bytes_exact'] = True
            result = ownership_screen(rows['candidate'], rows['baseline'])
            ratios = result['ratios_8_to_16']
            guards = result['guards']
            floors = []
            for arm in ARMS:
                parent = json.loads((pair_dir/arm/'parent-before-spawn.json').read_text())
                floors.append(not parent['pyarrow_imported'] and int(parent['process']['VmHWM'].split()[0])
                    < rows[arm]['process']['whole_process_peak_rss_kib'])
            guards['rss_parent_floor_below_worker'] = all(floors)
            pair = {'shape': shape, 'seed': seed, 'order': order, 'rows': rows, 'gates': gates,
                    'ratios_candidate_to_baseline': ratios, 'guards': guards,
                    'finite_seed_screen': all(guards.values())}
            mc.dump(pair_dir/'pair.json', pair)
            pairs.append(pair)
            if not all(gates.values()):
                raise RuntimeError('unchanged semantic gate failure')
            for state in states:
                shutil.rmtree(state)
            mc.dump(pair_dir/'cleanup.json', {'removed': all(not s.exists() for s in states),
                'retention': 'exact full member hashes, raw rows and physical checker before cleanup'})
        mc.dump(args.destination/'summary.json', {'stage': args.stage, 'seed': seed, 'pairs': pairs,
            'all_screens_passed': all(p['finite_seed_screen'] for p in pairs),
            'all_semantic_gates': all(all(p['gates'].values()) for p in pairs),
            'build_manifest_sha256': mc.digest(args.build_manifest)})
        shutil.rmtree(work)
        mc.dump(args.destination/'complete.json', {'exit_code': 0, 'pairs': 3, 'native_cells': 6,
            'scratch_removed': not work.exists(), 'qualification': False})
    except BaseException as error:
        sweep.preserve_failure(work, args.destination, error)
        raise


def main():
    mc.require_limits()
    # Adapt containment locations/ceilings in memory only; old runner files stay frozen.
    sweep.REGISTERED = REGISTERED
    sweep.EVIDENCE_LIMIT = 512*mc.MIB
    sweep.FAILURE_LIMIT = 192*mc.MIB
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['build', 'run'])
    parser.add_argument('--destination', type=Path, required=True)
    parser.add_argument('--protocol', type=Path, required=True)
    parser.add_argument('--binaries-root', type=Path)
    parser.add_argument('--build-manifest', type=Path)
    parser.add_argument('--stage', choices=SEEDS)
    parser.add_argument('--repeat', type=int, choices=[0, 1])
    parser.add_argument('--screen-receipts', type=Path, nargs=2)
    args = parser.parse_args()
    args.destination = args.destination.resolve()
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    if not scratch.is_relative_to(mc.DATA.resolve()) or args.destination.exists() or not args.destination.is_relative_to(REGISTERED.resolve()):
        parser.error('fresh registered evidence and owned data-drive scratch required')
    if args.mode == 'build' and args.binaries_root is None:
        parser.error('build requires binaries root')
    if args.mode == 'run' and (args.build_manifest is None or args.stage is None or args.repeat is None):
        parser.error('run requires build manifest, stage, repeat')
    args.destination.mkdir(parents=True)
    for source, name in ((PROPOSAL, 'proposal.txt'), (args.protocol, 'protocol.txt'), (Path(__file__), 'runner.py')):
        shutil.copyfile(source, args.destination/name)
    if args.mode == 'build':
        build(args)
    else:
        run(args)


if __name__ == '__main__':
    main()
