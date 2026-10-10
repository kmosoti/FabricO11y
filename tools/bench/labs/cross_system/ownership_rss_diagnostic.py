#!/usr/bin/env python3
"""Four fresh-parent frozen-input RSS diagnostic pairs; launcher required."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
from unittest.mock import patch

import memory_census as mc
import storage_sweep as sweep
from ownership_sweep import ownership_screen

BASE = mc.ROOT/'docs/experiments/benchmarks/data/native-frontier-01/memory'
ORIGIN = BASE/'confirmation-1/adversarial'
PROPOSAL = mc.ROOT/'docs/experiments/benchmarks/native-ownership-rss-diagnostic-proposal.md'
INPUT_SHA = 'c509bb4b79c262c931cb551e3e6912bd35fe94392c8e6323c3b9680322c6c210'


def counters(value):
    return sweep.io_counts(value) if value is not None else None


def classify(sample, before, after):
    if sample is None or before is None or after is None or 'wchar' not in sample or 'wchar' not in before or 'wchar' not in after:
        return 'unknown_missing_counter'
    a, b, now = before['wchar'], after['wchar'], sample['wchar']
    if b < a:
        raise ValueError('nonmonotonic native snapshot wchar')
    if now < a:
        return 'generation_approximate'
    if now == a or now == b:
        return 'transition_equality_ambiguous'
    if now < b:
        return 'build_writing_approximate'
    return 'post_build_approximate'


def stat_fields(value):
    if value is None:
        return None
    # comm can contain spaces/parentheses; fields after its last close start at3.
    end = value.rfind(')')
    if end < 0:
        raise ValueError('invalid /proc stat comm framing')
    fields = value[end+2:].split()
    if len(fields) < 22:
        raise ValueError('truncated /proc stat')
    return {'user_ticks': int(fields[11]), 'system_ticks': int(fields[12]),
            'virtual_bytes': int(fields[20]), 'resident_pages': int(fields[21])}


def controls():
    result = mc.controls()
    if classify({'wchar': 9}, {'wchar': 10}, {'wchar': 20}) != 'generation_approximate':
        raise AssertionError('generation counter classification')
    for now in (10, 20):
        if classify({'wchar': now}, {'wchar': 10}, {'wchar': 20}) != 'transition_equality_ambiguous':
            raise AssertionError('equal counter treated as exact phase')
    if classify({'wchar': 15}, {'wchar': 10}, {'wchar': 20}) != 'build_writing_approximate' or classify({'wchar': 21}, {'wchar': 10}, {'wchar': 20}) != 'post_build_approximate':
        raise AssertionError('build/postbuild counter classification')
    if classify(None, {}, {}) != 'unknown_missing_counter':
        raise AssertionError('missing counter assigned a phase')
    try:
        classify({'wchar': 1}, {'wchar': 20}, {'wchar': 10})
    except ValueError:
        result['decreasing_snapshot_counter_rejected'] = True
    else:
        raise AssertionError('decreasing snapshot accepted')
    fields = ['0']*22
    fields[11], fields[12], fields[20], fields[21] = '7', '8', '9000', '10'
    if stat_fields('12 (name with (spaces)) '+' '.join(fields)) != {
            'user_ticks': 7, 'system_ticks': 8, 'virtual_bytes': 9000, 'resident_pages': 10}:
        raise AssertionError('stat field index control')
    if stat_fields(None) is not None:
        raise AssertionError('missing stat assigned an endpoint')
    try:
        stat_fields('12 (name) R 1')
    except ValueError:
        result['truncated_stat_rejected'] = True
    else:
        raise AssertionError('truncated native snapshot accepted')
    result.update(transition_equality_ambiguous=True, missing_phase_unknown=True, stat_comm_spaces_handled=True)
    return result


def allocator_environment():
    names = sorted(k for k in os.environ if k.startswith('MALLOC_') or k in
                   ('GLIBC_TUNABLES', 'LD_PRELOAD', 'LD_AUDIT'))
    observed = {name: os.environ[name] for name in names}
    if observed:
        raise ValueError('inherited allocator/loader override must be absent: '+', '.join(names))
    return observed


def phase_summary(cell, row, page_size):
    before, after = counters(row['probe']['io_before']), counters(row['probe']['io_after'])
    groups = {}
    with (cell/'timeline.jsonl').open() as timeline:
        for line in timeline:
            sample = json.loads(line)
            label = classify(counters(sample.get('process_io')), before, after)
            process = sample.get('process') or {}
            compact = {'parent_elapsed_seconds': sample['elapsed_seconds'], 'process': process,
                       'published_segment_bytes': sample['disk_logical_bytes']['segment'],
                       'building_temporary_bytes': sample['disk_logical_bytes']['temporary']}
            entry = groups.setdefault(label, {'samples': 0, 'first': compact, 'last': compact,
                                             'maximum_sampled_rss_kib': None})
            entry['samples'] += 1
            entry['last'] = compact
            if process.get('VmRSS'):
                value = int(process['VmRSS'].split()[0])
                entry['maximum_sampled_rss_kib'] = max(value, entry['maximum_sampled_rss_kib'] or 0)
    start, finish = stat_fields(row['probe']['stat_before']), stat_fields(row['probe']['stat_after'])
    if start is None or finish is None:
        raise ValueError('native generation/build endpoint unavailable')
    return {'stat_before': start, 'stat_after': finish,
            'build_return_resident_bytes': finish['resident_pages']*page_size,
            'approximate_source_boundaries': groups,
            'sampled_hwm_not_monotonic_or_exact': True,
            'exclusive_phase_time_or_allocator_owner': 'not identified'}


def worker(args):
    allocator_environment()
    sweep.REGISTERED, sweep.EVIDENCE_LIMIT, sweep.FAILURE_LIMIT = BASE, 512*mc.MIB, 192*mc.MIB
    destination = args.destination.resolve()
    if destination.exists() or not destination.is_relative_to(BASE.resolve()):
        raise ValueError('fresh registered diagnostic pair directory required')
    manifest = json.loads(args.build_manifest.read_text())
    for name, digest in manifest['source_sha256'].items():
        if mc.digest(mc.ROOT/name) != digest:
            raise ValueError('frozen ownership source differs: '+name)
    if manifest['protocol_sha256'] != mc.digest(args.protocol):
        raise ValueError('frozen main protocol differs')
    destination.mkdir(parents=True)
    work = Path(tempfile.mkdtemp(prefix='ownership-rss-pair-', dir=os.environ['FABRIC_SCRATCH_ROOT']))
    deadline = time.monotonic()+args.seconds
    order = ['baseline', 'candidate'] if args.repeat == 0 else ['candidate', 'baseline']
    rows, states, phases = {}, [], {}
    try:
        for arm in order:
            sweep.guard(work, destination, deadline, reserve=True)
            record = manifest['binaries'][arm]
            binary = Path(record['path']).resolve()
            if not binary.is_relative_to(mc.DATA.resolve()) or mc.digest(binary) != record['sha256']:
                raise ValueError('frozen native binary differs')
            if record['build']['compile_env']['FABRIC_RUN_MIB_EXPERIMENT'] != '8' or record['build']['compile_env']['FABRIC_EARLY_ROW_RELEASE_EXPERIMENT'] != ('1' if arm == 'candidate' else None):
                raise ValueError('frozen ownership compile setting differs')
            cell = destination/arm
            cell.mkdir()
            state = work/arm
            states.append(state)
            original_popen = subprocess.Popen
            native_env = dict(os.environ)
            if args.policy == 'fixed128k':
                native_env['MALLOC_MMAP_THRESHOLD_'] = '131072'
            def spawn(*call_args, **kwargs):
                command = call_args[0] if call_args else kwargs.get('args')
                if isinstance(command, (list, tuple)) and command and Path(command[0]).resolve() == binary:
                    kwargs['env'] = native_env
                return original_popen(*call_args, **kwargs)
            mc.dump(cell/'native-environment.json', {'policy': args.policy,
                    'MALLOC_MMAP_THRESHOLD_': native_env.get('MALLOC_MMAP_THRESHOLD_'),
                    'inherited_overrides': allocator_environment(), 'scope': 'frozen native child only'})
            with patch.object(subprocess, 'Popen', spawn):
                rows[arm] = sweep.native(binary, state, cell, 'adversarial', 64, 2703204360,
                    'bounded', mc.cgroup(), work, destination, deadline)
            if rows[arm]['probe']['input_sha256'] != INPUT_SHA:
                raise ValueError('counterexample input digest differs')
            original_map = json.loads((ORIGIN/arm/'complete-member-map.json').read_text())
            new_map = json.loads((cell/'complete-member-map.json').read_text())
            mc.equal_member_maps(original_map, new_map)
            phases[arm] = phase_summary(cell, rows[arm], os.sysconf('SC_PAGE_SIZE'))
            mc.dump(cell/'phase-summary.json', phases[arm])
        gates = mc.grade(rows['baseline']['probe'], rows['candidate']['probe'])
        gates['exact_original_regular_member_maps'] = True
        costs = ownership_screen(rows['candidate'], rows['baseline'])
        floors = []
        for arm in ('baseline', 'candidate'):
            parent = json.loads((destination/arm/'parent-before-spawn.json').read_text())
            floors.append(not parent['pyarrow_imported'] and int(parent['process']['VmHWM'].split()[0])
                < rows[arm]['process']['whole_process_peak_rss_kib'])
        costs['guards']['rss_parent_floor_below_worker'] = all(floors)
        pair = {'policy': args.policy, 'repeat': args.repeat, 'order': order,
                'seed': 2703204360, 'shape': 'adversarial', 'target_mib': 64,
                'rows': rows, 'gates': gates, 'phase_summaries': phases,
                'build_return_resident_gap_bytes': phases['candidate']['build_return_resident_bytes']-phases['baseline']['build_return_resident_bytes'],
                'whole_native_rss_ratio': rows['candidate']['process']['whole_process_peak_rss_kib']/rows['baseline']['process']['whole_process_peak_rss_kib'],
                'cost_ratios_candidate_to_baseline': costs['ratios_8_to_16'],
                'original_cost_guards_descriptive': costs['guards'],
                'original_confirmation_still_failed': True,
                'origin_sha256': {str(p.relative_to(mc.ROOT)): mc.digest(p) for arm in ('baseline', 'candidate')
                    for p in (ORIGIN/arm/'complete-member-map.json', ORIGIN/'pair.json')},
                'build_manifest_sha256': mc.digest(args.build_manifest)}
        mc.dump(destination/'pair.json', pair)
        if not all(gates.values()):
            raise ValueError('unchanged semantic gate failure')
        for state in states:
            shutil.rmtree(state)
        shutil.rmtree(work)
        mc.dump(destination/'complete.json', {'exit_code': 0, 'native_cells': 2,
                'scratch_removed': not work.exists(), 'cost_metrics_are_descriptive': True})
    except BaseException as error:
        sweep.preserve_failure(work, destination, error)
        raise


def main():
    mc.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--destination', type=Path, required=True)
    parser.add_argument('--protocol', type=Path, required=True)
    parser.add_argument('--build-manifest', type=Path, required=True)
    parser.add_argument('--worker', action='store_true', help=argparse.SUPPRESS)
    parser.add_argument('--policy', choices=['default', 'fixed128k'])
    parser.add_argument('--repeat', type=int, choices=[0, 1])
    parser.add_argument('--seconds', type=float, default=120)
    args = parser.parse_args()
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    if not scratch.is_relative_to(mc.DATA.resolve()):
        raise ValueError('owned data-drive scratch required')
    if args.worker:
        if args.policy is None or args.repeat is None or not 0 < args.seconds <= 540:
            raise ValueError('worker requires finite policy/repeat')
        worker(args)
        return
    environment = allocator_environment()
    destination = args.destination.resolve()
    if destination.exists() or not destination.is_relative_to(BASE.resolve()):
        raise ValueError('fresh registered diagnostic evidence required')
    destination.mkdir(parents=True)
    for source, name in ((Path(__file__), 'runner.py'), (PROPOSAL, 'proposal.txt'), (args.protocol, 'protocol.txt')):
        shutil.copyfile(source, destination/name)
    mc.dump(destination/'negative-controls.json', controls())
    mc.dump(destination/'metadata.json', {'runner_sha256': mc.digest(Path(__file__)),
        'proposal_sha256': mc.digest(PROPOSAL), 'protocol_sha256': mc.digest(args.protocol),
        'libc': os.confstr('CS_GNU_LIBC_VERSION'), 'page_size': os.sysconf('SC_PAGE_SIZE'),
        'clock_ticks_per_second': os.sysconf('SC_CLK_TCK'), 'inherited_overrides': environment,
        'input_sha256': INPUT_SHA, 'qualification': False})
    decoder = Path(tempfile.mkdtemp(prefix='ownership-rss-decoder-', dir=scratch))
    receipt = {'path': str(decoder), 'scope': 'replaceable isolated validation dependency'}
    deadline = time.monotonic()+540
    try:
        env = dict(os.environ, UV_PYTHON_DOWNLOADS='never', PYTHONDONTWRITEBYTECODE='1')
        command = ['uv', 'pip', 'install', '--no-cache', '--only-binary', ':all:', '--python', sys.executable,
                   '--target', str(decoder), 'pyarrow==22.0.0']
        receipt['install_command'] = command
        result = subprocess.run(command, env=env, timeout=45)
        receipt['install_exit_code'] = result.returncode
        result.check_returncode()
        receipt['package_metadata_sha256'] = {str(p.relative_to(decoder)): mc.digest(p)
            for p in decoder.glob('*.dist-info/*') if p.is_file()}
        env['PYTHONPATH'] = str(decoder)
        pairs = []
        for policy in ('default', 'fixed128k'):
            for repeat in (0, 1):
                pair_dir = destination/f'{policy}-{repeat}'
                remaining = deadline-time.monotonic()
                if remaining <= 0:
                    raise RuntimeError('540-second diagnostic deadline')
                command = [sys.executable, '-B', str(Path(__file__).resolve()), '--worker',
                    '--destination', str(pair_dir), '--protocol', str(args.protocol),
                    '--build-manifest', str(args.build_manifest), '--policy', policy,
                    '--repeat', str(repeat), '--seconds', str(remaining)]
                mc.dump(destination/f'{policy}-{repeat}-command.json', {'argv': command})
                result = subprocess.run(command, env=env, timeout=remaining)
                mc.dump(destination/f'{policy}-{repeat}-worker.json', {'exit_code': result.returncode})
                result.check_returncode()
                pairs.append(json.loads((pair_dir/'pair.json').read_text()))
        defaults = [p['build_return_resident_gap_bytes'] for p in pairs if p['policy']=='default']
        fixed = [p['build_return_resident_gap_bytes'] for p in pairs if p['policy']=='fixed128k']
        supported = min(defaults)>0 and all(abs(gap) <= .5*min(defaults) for gap in fixed)
        mc.dump(destination/'summary.json', {'pairs': pairs,
            'threshold_sensitive_residency_supported': supported,
            'original_default_rss_excess_repeated_both_orders': all(p['whole_native_rss_ratio']>1.1 for p in pairs if p['policy']=='default'),
            'allocation_owner_or_dynamic_threshold_trajectory': 'not identified',
            'original_confirmation_still_failed': True})
        mc.dump(destination/'complete.json', {'exit_code': 0, 'pairs': 4, 'native_cells': 8,
            'qualification': False, 'original_confirmation_still_failed': True})
    finally:
        shutil.rmtree(decoder)
        receipt['removed'] = not decoder.exists()
        mc.dump(destination/'decoder-cleanup.json', receipt)


if __name__ == '__main__':
    main()
