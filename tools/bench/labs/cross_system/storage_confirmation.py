#!/usr/bin/env python3
"""Prospective fresh-seed bounded8/bounded16 confirmation; launcher only."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

import memory_census as mc
import storage_sweep as sweep

PROPOSAL = mc.ROOT / 'docs/experiments/benchmarks/cross-system-storage-confirmation-proposal.md'
SEEDS = (2703204355, 2703204356)


def screen(left, right):
    def metrics(row):
        probe, process = row['probe'], row['process']
        return {'heap': probe['peak_live_heap_bytes'],
                'throughput': probe['encoded_group_bytes']/probe['build_seconds_instrumented'],
                'write': sweep.io_delta(probe['io_before'], probe['io_after'])['write_bytes'],
                'cpu': process['user_seconds']+process['system_seconds'],
                'rss': process['whole_process_peak_rss_kib']}
    a, b = metrics(left), metrics(right)
    if any(b[key] <= 0 or a[key] <= 0 for key in ('heap', 'throughput', 'cpu', 'rss')):
        raise ValueError('nonpositive heap/throughput/CPU/RSS measurement')
    if a['write'] < 0 or b['write'] < 0:
        raise ValueError('negative build write measurement')
    ratios = {key: a[key]/b[key] if b[key] > 0 else None for key in a}
    guards = {'benefit_at_least_10_percent': ratios['heap'] <= .9 or
              (ratios['write'] is not None and ratios['write'] <= .9),
              'throughput_at_least_90_percent': ratios['throughput'] >= .9,
              'native_cpu_at_most_110_percent': ratios['cpu'] <= 1.1,
              'native_rss_at_most_110_percent': ratios['rss'] <= 1.1,
              'useful_work_observation_differs': ratios['heap'] <= .9}
    return {'ratios_8_to_16': ratios, 'guards': guards}


def controls():
    result = sweep.controls()
    def row(heap=100, seconds=1, cpu=1, rss=100):
        return {'probe': {'peak_live_heap_bytes': heap, 'encoded_group_bytes': 100,
                'build_seconds_instrumented': seconds,
                'io_before': 'write_bytes: 0\n', 'io_after': 'write_bytes: 100\n'},
                'process': {'user_seconds': cpu, 'system_seconds': 0,
                            'whole_process_peak_rss_kib': rss}}
    base = row()
    if not all(screen(row(heap=80), base)['guards'].values()):
        raise AssertionError('representative benefit rejected')
    for name, defect in [('null_benefit', row()), ('slow', row(heap=80, seconds=2)),
                         ('cpu', row(heap=80, cpu=2)), ('rss', row(heap=80, rss=200))]:
        if all(screen(defect, base)['guards'].values()):
            raise AssertionError('screen defect accepted: '+name)
        result[name+'_screen_rejected'] = True
    zero_base, zero_gain, zero_null = row(), row(heap=80), row()
    for value in (zero_base, zero_gain, zero_null):
        value['probe']['io_after'] = 'write_bytes: 0\n'
    gain = screen(zero_gain, zero_base)
    if gain['ratios_8_to_16']['write'] is not None or not all(gain['guards'].values()):
        raise AssertionError('zero-write heap benefit/unknown IO control')
    if all(screen(zero_null, zero_base)['guards'].values()):
        raise AssertionError('zero-write null benefit accepted')
    result['zero_write_is_unknown_heap_gain_valid_null_rejected'] = True
    return result


def worker(args):
    destination = args.destination.resolve()
    if destination.exists() or not destination.is_relative_to(sweep.REGISTERED.resolve()):
        raise ValueError('fresh registered memory evidence destination required')
    manifest = json.loads(args.build_manifest.read_text())
    # Bind exact previously frozen files, not newly generated source inventories.
    for name, digest in manifest['source_sha256'].items():
        if mc.digest(mc.ROOT / name) != digest:
            raise ValueError('frozen source changed: '+name)
    if mc.digest(args.protocol) != manifest['protocol_sha256']:
        raise ValueError('frozen storage protocol differs')
    binaries = {}
    for cap in (8, 16):
        record = manifest['binaries'][str(cap)]
        binary = Path(record['path']).resolve()
        if not binary.is_relative_to(mc.DATA.resolve()) or mc.digest(binary) != record['sha256']:
            raise ValueError('frozen binary changed')
        if record['build']['compile_env']['FABRIC_RUN_MIB_EXPERIMENT'] != str(cap):
            raise ValueError('compile cap mismatch')
        binaries[cap] = binary
    destination.mkdir(parents=True)
    for source, name in ((PROPOSAL, 'proposal.txt'), (args.protocol, 'protocol.txt'),
                         (Path(__file__), 'runner.py')):
        shutil.copyfile(source, destination / name)
    targets = [args.target_mib] if args.target_mib else [16, 64]
    deadline = time.monotonic()+(840 if args.target_mib else 1200)
    mc.dump(destination / 'negative-controls.json', controls())
    mc.dump(destination / 'metadata.json', {'targets_mib': targets, 'seeds': SEEDS,
            'caps_mib': [8, 16], 'binaries': manifest['binaries'],
            'build_manifest_sha256': mc.digest(args.build_manifest),
            'source_sha256': manifest['source_sha256'],
            'protocol_sha256': mc.digest(args.protocol), 'proposal_sha256': mc.digest(PROPOSAL),
            'runner_sha256': mc.digest(Path(__file__)), **mc.decoder_call('version', deadline),
            'qualification': False, 'reference_arm': 'bounded 16 MiB; no full-sort reference rerun',
            'ledger_provenance': 'unchanged Rust differential scanner; independent Python physical FTF1'})
    work = Path(tempfile.mkdtemp(prefix='storage-confirmation-', dir=os.environ['FABRIC_SCRATCH_ROOT']))
    group, pairs = mc.cgroup(), []
    try:
        for target in targets:
            for repeat, seed in enumerate(SEEDS):
                for index, shape in enumerate(sweep.SHAPES):
                    sweep.guard(work, destination, deadline, reserve=True)
                    pair_dir = destination / f'target-{target}-seed-{seed}-{shape}'
                    pair_dir.mkdir()
                    order = [8, 16] if (index+repeat) % 2 == 0 else [16, 8]
                    rows, states = {}, []
                    for cap in order:
                        cell = pair_dir / f'run-{cap}'
                        cell.mkdir()
                        state = work / f'{pair_dir.name}-run-{cap}'
                        states.append(state)
                        rows[str(cap)] = sweep.native(binaries[cap], state, cell, shape,
                            target, seed, 'bounded', group, work, destination, deadline)
                    gates = mc.grade(rows['16']['probe'], rows['8']['probe'])
                    result = screen(rows['8'], rows['16'])
                    floors = []
                    for cap in (8, 16):
                        parent = json.loads((pair_dir / f'run-{cap}/parent-before-spawn.json').read_text())
                        floors.append(not parent['pyarrow_imported'] and
                            int(parent['process']['VmHWM'].split()[0]) < rows[str(cap)]['process']['whole_process_peak_rss_kib'])
                    result['guards']['rss_parent_floor_below_worker'] = all(floors)
                    pair = {'target_mib': target, 'shape': shape, 'seed': seed,
                            'order': order, 'rows': rows, 'gates': gates, **result,
                            'finite_seed_screen': all(result['guards'].values())}
                    mc.dump(pair_dir / 'pair.json', pair)
                    pairs.append(pair)
                    if not all(gates.values()):
                        raise RuntimeError('unchanged semantic gate failed')
                    for state in states:
                        shutil.rmtree(state)
                    mc.dump(pair_dir / 'cleanup.json', {'removed': all(not s.exists() for s in states),
                        'retention': 'complete regular content maps + raw rows/costs; independently validated before cleanup'})
        screens = [{'target_mib': target, 'shape': shape,
                    'both_new_seeds_confirmed': all(p['finite_seed_screen'] for p in pairs
                        if p['target_mib'] == target and p['shape'] == shape)}
                   for target in targets for shape in sweep.SHAPES]
        mc.dump(destination / 'summary.json', {'pairs': pairs, 'screens': screens,
                'all_requested_screens_confirmed': all(s['both_new_seeds_confirmed'] for s in screens)})
        shutil.rmtree(work)
        mc.dump(destination / 'complete.json', {'exit_code': 0, 'pairs': len(pairs),
                'native_cells': 2*len(pairs), 'scratch_removed': not work.exists(),
                'screen_failure_is_research_outcome': True, 'qualification': False})
    except BaseException as error:
        sweep.preserve_failure(work, destination, error)
        raise


def main():
    mc.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--destination', type=Path, required=True)
    parser.add_argument('--protocol', type=Path, required=True)
    parser.add_argument('--build-manifest', type=Path, required=True)
    parser.add_argument('--target-mib', type=int, choices=[16, 64])
    parser.add_argument('--worker', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    if not scratch.is_relative_to(mc.DATA.resolve()):
        raise ValueError('owned data-drive scratch required')
    if args.worker:
        worker(args)
        return
    destination = args.destination.resolve()
    receipt_path = destination.parent / (destination.name+'-decoder.json')
    if destination.exists() or receipt_path.exists() or not destination.is_relative_to(sweep.REGISTERED.resolve()):
        raise ValueError('fresh registered destination and decoder receipt required')
    destination.parent.mkdir(parents=True, exist_ok=True)
    decoder = Path(tempfile.mkdtemp(prefix='confirmation-decoder-', dir=scratch))
    env = dict(os.environ, UV_PYTHON_DOWNLOADS='never', PYTHONDONTWRITEBYTECODE='1')
    command = ['uv', 'pip', 'install', '--no-cache', '--only-binary', ':all:',
               '--python', sys.executable, '--target', str(decoder), 'pyarrow==22.0.0']
    receipt = {'install_command': command, 'decoder': str(decoder)}
    try:
        result = subprocess.run(command, env=env, timeout=45)
        receipt['install_exit_code'] = result.returncode
        result.check_returncode()
        receipt['package_metadata_sha256'] = {str(p.relative_to(decoder)): mc.digest(p)
                for p in decoder.glob('*.dist-info/*') if p.is_file()}
        env['PYTHONPATH'] = str(decoder)
        command = [sys.executable, '-B', str(Path(__file__).resolve()), '--worker',
                   '--destination', str(destination), '--protocol', str(args.protocol),
                   '--build-manifest', str(args.build_manifest)]
        if args.target_mib:
            command += ['--target-mib', str(args.target_mib)]
        receipt['worker_command'] = command
        mc.dump(receipt_path, receipt)
        result = subprocess.run(command, env=env, timeout=850 if args.target_mib else 1210)
        receipt['worker_exit_code'] = result.returncode
        result.check_returncode()
    finally:
        shutil.rmtree(decoder)
        receipt['decoder_removed'] = not decoder.exists()
        mc.dump(receipt_path, receipt)


if __name__ == '__main__':
    main()
