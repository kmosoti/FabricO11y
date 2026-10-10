#!/usr/bin/env python3
"""Finite external-sort run-size sweep; invoke only under resource_group/run_job.

Passing states retain complete content maps, not full fixture archives. The
unchanged memory census supplies differential gates and an isolated decoder.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import time

import memory_census as mc

ROOT = mc.ROOT
MIB = mc.MIB
CAPS = (8, 16, 32)
SHAPES = ('steady', 'adversarial', 'bigrows')
SEEDS = (2703204353, 2703204354)
RAW_LIMIT = 512 * MIB
EVIDENCE_LIMIT = 256 * MIB
FAILURE_LIMIT = 128 * MIB
CAMPAIGN_SECONDS = 840  # Leaves decoder installation/cleanup inside a 900s outer job.
REGISTERED = ROOT / 'docs/experiments/benchmarks/data/cross-system-sweep-01/memory'
PROPOSAL = ROOT / 'docs/experiments/benchmarks/cross-system-storage-sweep-proposal.md'
EXPERIMENT_ENV = ('FABRIC_RUN_MIB_EXPERIMENT', 'FABRIC_SPILL_WORKSPACE_EXPERIMENT',
                  'FABRIC_BORROWED_LOG_EXPERIMENT')


def footprint(root):
    """Charge logical and allocated regular bytes; reject links/special files."""
    total = 0
    for path in root.rglob('*'):
        try:
            info = path.lstat()
        except FileNotFoundError:
            continue
        if stat.S_ISREG(info.st_mode):
            total += max(info.st_size, info.st_blocks * 512)
        elif not stat.S_ISDIR(info.st_mode):
            raise RuntimeError('nonregular owned member: ' + str(path))
    return total


def evidence_root(destination):
    return REGISTERED if destination.is_relative_to(REGISTERED) else destination.parent


def source_identity():
    sources = [mc.PROBE, ROOT / 'Cargo.lock', PROPOSAL, Path(__file__), Path(mc.__file__),
               Path(__file__).with_name('storage_with_decoder.py')]
    for base in ('crates/fabric-server/src', 'crates/fabric-frame/src'):
        sources += sorted((ROOT / base).rglob('*.rs'))
    return {str(path.relative_to(ROOT)): mc.digest(path) for path in sources}


def guard(work, destination, deadline, reserve=False):
    if time.monotonic() >= deadline:
        raise RuntimeError('840-second inner campaign deadline')
    if footprint(work) > RAW_LIMIT:
        raise RuntimeError('512MiB whole-pair scratch ceiling; preserve before cleanup')
    used = footprint(evidence_root(destination))
    if used > EVIDENCE_LIMIT or (reserve and used + FAILURE_LIMIT > EVIDENCE_LIMIT):
        raise RuntimeError('256MiB evidence/128MiB failure reserve unavailable')
    if shutil.disk_usage(work).free < mc.RESERVE:
        raise RuntimeError('16GiB data-drive free reserve unavailable')


def io_counts(value):
    if value is None:
        return None
    return {line.split(':', 1)[0]: int(line.split(':', 1)[1]) for line in value.splitlines()}


def io_delta(before, after):
    left, right = io_counts(before), io_counts(after)
    if left is None or right is None:
        return None
    if left.keys() != right.keys() or any(right[k] < left[k] for k in left):
        raise ValueError('nonmonotonic/mismatched process IO counters')
    return {key: right[key]-left[key] for key in left}


def run_files(state):
    files = {}
    for path in state.rglob('*.run-*'):
        match = re.fullmatch(r'(.+)\.run-(\d+)-(\d+)', path.name)
        if match is None:
            continue
        try:
            info = path.stat()
        except FileNotFoundError:
            continue
        if stat.S_ISREG(info.st_mode):
            files[str(path.relative_to(state))] = {'table': match[1], 'level': int(match[2]),
                    'index': int(match[3]), 'bytes': info.st_size}
    return files


def preserve_failure(work, destination, error):
    members = mc.regular_members(work)
    raw = footprint(work)
    mc.dump(destination / 'failure-members.json', members)
    receipt = {'error': str(error), 'scratch': str(work), 'scratch_bytes': raw,
               'scratch_removed': False, 'complete_archive': False}
    archive = destination / 'failed-state.tar.gz'
    try:
        remaining = EVIDENCE_LIMIT - footprint(evidence_root(destination)) - MIB
        if raw > RAW_LIMIT or remaining <= 0:
            raise RuntimeError('failed state exceeds preservation envelope')
        with archive.open('xb') as output:
            with tarfile.open(fileobj=mc.ArchiveWriter(output, min(FAILURE_LIMIT, remaining)),
                              mode='w:gz', compresslevel=1) as stream:
                stream.add(work, arcname=work.name, recursive=True)
        observed = {}
        decoded = 0
        with tarfile.open(archive, 'r:gz') as stream:
            for entry in stream:
                parts = Path(entry.name).parts
                if not parts or parts[0] != work.name or '..' in parts:
                    raise ValueError('archive member root mismatch')
                if entry.isdir():
                    continue
                if not entry.isfile():
                    raise ValueError('nonregular archive member')
                name = str(Path(*parts[1:]))
                if name in observed:
                    raise ValueError('duplicate archive member')
                decoded += entry.size
                if decoded > RAW_LIMIT:
                    raise ValueError('archive decoded ceiling')
                reader = stream.extractfile(entry)
                h = hashlib.sha256()
                count = 0
                for chunk in iter(lambda: reader.read(MIB), b''):
                    count += len(chunk)
                    h.update(chunk)
                observed[name] = {'bytes': count, 'sha256': h.hexdigest()}
        mc.equal_member_maps(members, observed)
        mc.equal_member_maps(members, mc.regular_members(work))
        receipt.update(complete_archive=True, payload_verification='all regular paths/bytes/sha256 read back',
                       archive_sha256=mc.digest(archive), archive_bytes=archive.stat().st_size)
        shutil.rmtree(work)
        receipt['scratch_removed'] = not work.exists()
    except BaseException as preservation_error:
        receipt['preservation_error'] = str(preservation_error)
        if archive.exists():
            receipt.update(partial_archive_bytes=archive.stat().st_size,
                           partial_archive_sha256=mc.digest(archive))
    mc.dump(destination / 'failure.json', receipt)


def native(binary, state, cell, shape, target, seed, builder, group, work, destination, deadline):
    argv = [str(binary), str(state), shape, str(target), str(seed), builder]
    mc.dump(cell / 'command.json', {'argv': argv})
    started = time.monotonic()
    child = None
    usage = None
    code = None
    failure = None
    row = None
    wall = None
    observed_runs = {}
    try:
        mc.dump(cell / 'parent-before-spawn.json', {'pid': os.getpid(),
                'process': mc.sample(group, os.getpid(), state, started)['process'],
                'pyarrow_imported': 'pyarrow' in sys.modules})
        if 'pyarrow' in sys.modules:
            raise RuntimeError('native-spawning parent imported decoder')
        with (cell / 'stdout.json').open('wb') as out, (cell / 'stderr.txt').open('wb') as err, \
                (cell / 'timeline.jsonl').open('w') as timeline:
            child = subprocess.Popen(argv, stdout=out, stderr=err)
            while True:
                observation = mc.sample(group, child.pid, state, started)
                observation['run_files'] = run_files(state)
                observed_runs.update(observation['run_files'])
                try:
                    observation['process_io'] = Path(f'/proc/{child.pid}/io').read_text()
                except FileNotFoundError:
                    observation['process_io'] = None
                timeline.write(json.dumps(observation)+'\n')
                timeline.flush()
                pid, status, usage = os.wait4(child.pid, os.WNOHANG)
                if pid:
                    code = child.returncode = os.waitstatus_to_exitcode(status)
                    wall = time.monotonic()-started
                    break
                guard(work, destination, deadline)
                if time.monotonic()-started > mc.TRIAL_SECONDS:
                    raise RuntimeError('120-second native child deadline')
                if out.tell()+err.tell() > MIB:
                    raise RuntimeError('1MiB native output ceiling')
                time.sleep(mc.SAMPLE_SECONDS)
        if code != 0:
            raise RuntimeError('native child exit ' + str(code))
        row = json.loads((cell / 'stdout.json').read_text())
        expected = {'shape': shape, 'target_mib': target, 'seed': seed, 'builder': builder}
        if any(row.get(key) != value for key, value in expected.items()):
            raise ValueError('native reported fixture/arm differs from command')
        row['physical_filter_check'] = mc.decoder_call('validate', deadline, state, cell / 'stdout.json')
        mc.dump(cell / 'physical-filter-check.json', row['physical_filter_check'])
        # Native verify + independent authentication/FTF1 row reconstruction precede deletion.
        mc.dump(cell / 'complete-member-map.json', mc.regular_members(state))
        build_seconds = row['build_seconds_instrumented']
        if not build_seconds > 0:
            raise ValueError('nonpositive build time')
        mc.dump(cell / 'costs.json', {'build_io_delta': io_delta(row['io_before'], row['io_after']),
                'build_encoded_mib_per_second': row['encoded_group_bytes']/MIB/build_seconds,
                'build_journal_mib_per_second': row['journal_bytes']/MIB/build_seconds,
                'whole_native_encoded_mib_per_second': row['encoded_group_bytes']/MIB/wall,
                'disk_final_logical_bytes': mc.inventory(state),
                'observed_run_files': observed_runs,
                'level0_spill_count_lower_bound': {table: max(
                    [entry['index']+1 for entry in observed_runs.values()
                     if entry['table'] == table and entry['level'] == 0], default=0)
                    for table in ['logs', 'metrics', 'spans']},
                'intermediate_merge_observed': any(entry['level'] > 0 for entry in observed_runs.values()),
                'phase_metrics': 'unsupported: readiness probe does not install/take phase observer',
                'run_observation_boundary': '100ms filesystem samples; misses short-lived files; counts are lower bounds'})
    except BaseException as error:
        failure = error
        if child is not None and child.returncode is None:
            child.kill()
            _, status, usage = os.wait4(child.pid, 0)
            code = child.returncode = os.waitstatus_to_exitcode(status)
            wall = time.monotonic()-started
    finally:
        receipt = {'exit_code': code, 'wall_seconds': wall,
                   'user_seconds': usage.ru_utime if usage else None,
                   'system_seconds': usage.ru_stime if usage else None,
                   'whole_process_peak_rss_kib': usage.ru_maxrss if usage else None,
                   'error': str(failure) if failure else None}
        mc.dump(cell / 'process.json', receipt)
    if failure:
        raise failure
    return {'probe': row, 'process': receipt}


def controls():
    result = mc.controls()
    if io_delta('write_bytes: 9\nwchar: 12\n', 'write_bytes: 13\nwchar: 20\n') != {
            'write_bytes': 4, 'wchar': 8}:
        raise AssertionError('IO delta control')
    try:
        io_delta('write_bytes: 9\n', 'write_bytes: 8\n')
    except ValueError:
        result['io_counter_reset_rejected'] = True
    else:
        raise AssertionError('decreasing IO counter accepted')
    return result


def build(args):
    binaries = args.binaries_root.resolve()
    if not binaries.is_relative_to(mc.DATA.resolve()) or binaries.exists():
        raise ValueError('fresh data-drive binary directory required')
    binaries.mkdir(parents=True)
    env = dict(os.environ)
    env['CARGO_BUILD_JOBS'] = '2'
    for name in EXPERIMENT_ENV:
        env.pop(name, None)
    records = {}
    identity = source_identity()
    for cap in CAPS:
        env['FABRIC_RUN_MIB_EXPERIMENT'] = str(cap)
        command = ['cargo', 'build', '--offline', '--locked', '--release', '-p', 'fabric-server',
                   '--example', 'readiness_memory_lab']
        started = time.monotonic()
        with (args.destination / f'build-{cap}.log').open('wb') as log:
            result = subprocess.run(command, cwd=ROOT, env=env, stdout=log,
                                    stderr=subprocess.STDOUT, timeout=360)
        receipt = {'argv': command, 'compile_env': {name: env.get(name) for name in EXPERIMENT_ENV},
                   'cargo_build_jobs': 2,
                   'rustflags': env.get('RUSTFLAGS'), 'exit_code': result.returncode,
                   'elapsed_seconds': time.monotonic()-started}
        mc.dump(args.destination / f'build-{cap}.json', receipt)
        result.check_returncode()
        source = Path(os.environ['CARGO_TARGET_DIR']) / 'release/examples/readiness_memory_lab'
        frozen = binaries / f'readiness-memory-run-{cap}'
        shutil.copyfile(source, frozen)
        frozen.chmod(0o755)
        records[str(cap)] = {'path': str(frozen), 'sha256': mc.digest(frozen), 'build': receipt}
    if source_identity() != identity:
        raise ValueError('source changed during builds')
    mc.dump(args.destination / 'build-manifest.json', {'binaries': records, 'source_sha256': identity,
             'protocol_sha256': mc.digest(args.protocol), 'proposal_sha256': mc.digest(PROPOSAL),
             'scope': 'existing compile-only run selector; no defaults changed'})
    mc.dump(args.destination / 'complete.json', {'exit_code': 0, 'build_count': 3,
             'binary_cache_retained': str(binaries), 'qualification': False})


def run(args):
    manifest = json.loads(args.build_manifest.read_text())
    if manifest['source_sha256'] != source_identity() or manifest['protocol_sha256'] != mc.digest(args.protocol):
        raise ValueError('source/protocol differs from frozen build provenance')
    binaries = {}
    for cap in CAPS:
        record = manifest['binaries'][str(cap)]
        path = Path(record['path']).resolve()
        if not path.is_relative_to(mc.DATA.resolve()) or mc.digest(path) != record['sha256']:
            raise ValueError('binary path/content differs from frozen build')
        if record['build']['compile_env']['FABRIC_RUN_MIB_EXPERIMENT'] != str(cap):
            raise ValueError('run-size compile setting mismatch')
        binaries[cap] = path
    deadline = time.monotonic()+CAMPAIGN_SECONDS
    decoder = mc.decoder_call('version', deadline)
    mc.dump(args.destination / 'negative-controls.json', controls())
    mc.dump(args.destination / 'metadata.json', {'target_mib': args.target_mib, 'repeat': args.repeat,
            'seed': SEEDS[args.repeat], 'caps_mib': CAPS, 'shapes': SHAPES,
            'source_sha256': manifest['source_sha256'], 'build_manifest_sha256': mc.digest(args.build_manifest),
            'protocol_sha256': mc.digest(args.protocol), 'binaries': manifest['binaries'],
            'proposal_sha256': mc.digest(PROPOSAL),
            **decoder, 'sampling_seconds': mc.SAMPLE_SECONDS,
            'cpu_affinity': sorted(os.sched_getaffinity(0)), 'kernel': os.uname().release,
            'ledger_provenance': 'same Rust scanners differential; not independent row oracle',
            'cap_model': {'row_limit': 32768, 'condition': 'nonempty AND (rows>=32768 OR resident+next>cap)',
                          'resident_bytes': 'Rust size_of row + actual String capacities + map allowances',
                          'constant_row_model': 'min(32768,floor(cap_bytes/row_resident_bytes)); oversized row exception',
                          'body_only_fit_upper_bound': {shape: {str(cap): min(32768, cap*MIB//(
                              16384 if shape == 'bigrows' else 512)) for cap in CAPS} for shape in SHAPES},
                          'body_bytes_by_shape': {'steady': 512, 'adversarial': 512, 'bigrows': 16384}},
            'physical_filter_provenance': 'unchanged independent Python FTF1 + PyArrow reconstruction',
            'reservation_metric': 'unsupported', 'qualification': False,
            'limits': {'scratch_bytes': RAW_LIMIT, 'aggregate_evidence_bytes': EVIDENCE_LIMIT,
                       'failure_archive_bytes': FAILURE_LIMIT, 'free_reserve_bytes': mc.RESERVE,
                       'child_seconds': mc.TRIAL_SECONDS, 'campaign_seconds': CAMPAIGN_SECONDS}})
    work = Path(tempfile.mkdtemp(prefix='storage-sweep-', dir=os.environ['FABRIC_SCRATCH_ROOT']))
    group = mc.cgroup()
    pairs = []
    try:
        for cap_index, cap in enumerate(CAPS):
            for shape_index, shape in enumerate(SHAPES):
                guard(work, args.destination, deadline, reserve=True)
                pair_name = f'run-{cap}-{shape}'
                pair_dir = args.destination / pair_name
                pair_dir.mkdir()
                rows = {}
                order = ['reference', 'bounded']
                if (cap_index+shape_index+args.repeat) % 2:
                    order.reverse()
                states = []
                for builder in order:
                    guard(work, args.destination, deadline)
                    cell = pair_dir / builder
                    cell.mkdir()
                    state = work / f'{pair_name}-{builder}'
                    states.append(state)
                    rows[builder] = native(binaries[cap], state, cell, shape, args.target_mib,
                                          SEEDS[args.repeat], builder, group, work, args.destination, deadline)
                gates = mc.grade(rows['reference']['probe'], rows['bounded']['probe'])
                pair = {'run_mib': cap, 'shape': shape, 'target_mib': args.target_mib,
                        'repeat': args.repeat, 'seed': SEEDS[args.repeat], 'order': order,
                        'rows': rows, 'gates': gates}
                mc.dump(pair_dir / 'pair.json', pair)
                pairs.append(pair)
                if not all(gates.values()):
                    raise RuntimeError('unchanged differential gate failure: ' + pair_name + ': ' + str(gates))
                # Both arms remain available until their paired semantic gates succeed.
                for state in states:
                    shutil.rmtree(state)
                mc.dump(pair_dir / 'cleanup.json', {'removed': all(not state.exists() for state in states),
                        'passing_fixture_retention': 'complete regular-member map + native raw rows summary; no archive'})
                guard(work, args.destination, deadline)
        mc.dump(args.destination / 'summary.json', {'pairs': pairs})
        shutil.rmtree(work)
        mc.dump(args.destination / 'complete.json', {'exit_code': 0, 'pairs': len(pairs),
                'native_cells': 2*len(pairs), 'scratch_removed': not work.exists(), 'qualification': False})
    except BaseException as error:
        preserve_failure(work, args.destination, error)
        raise


def main():
    mc.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['build', 'run', 'controls'])
    parser.add_argument('--destination', type=Path, required=True)
    parser.add_argument('--protocol', type=Path, required=True)
    parser.add_argument('--binaries-root', type=Path)
    parser.add_argument('--build-manifest', type=Path)
    parser.add_argument('--target-mib', type=int, choices=[16, 64])
    parser.add_argument('--repeat', type=int, choices=[0, 1])
    args = parser.parse_args()
    args.destination = args.destination.resolve()
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    if not scratch.is_relative_to(mc.DATA.resolve()):
        parser.error('owned scratch must use data drive')
    if not (args.destination.is_relative_to(REGISTERED.resolve()) or
            args.destination.is_relative_to(mc.DATA.resolve())) or args.destination.exists():
        parser.error('fresh registered storage/data-drive evidence directory required')
    if args.mode == 'build' and args.binaries_root is None:
        parser.error('build requires --binaries-root')
    if args.mode == 'run' and (args.build_manifest is None or args.target_mib is None or args.repeat is None):
        parser.error('run requires build manifest, target MiB and repeat')
    args.destination.mkdir(parents=True)
    shutil.copyfile(args.protocol, args.destination / 'protocol.txt')
    shutil.copyfile(PROPOSAL, args.destination / 'proposal.txt')
    shutil.copyfile(__file__, args.destination / 'runner.py')
    if args.mode == 'controls':
        mc.dump(args.destination / 'negative-controls.json', controls())
        mc.dump(args.destination / 'complete.json', {'exit_code': 0, 'controls_only': True})
    elif args.mode == 'build':
        build(args)
    else:
        run(args)


if __name__ == '__main__':
    main()
