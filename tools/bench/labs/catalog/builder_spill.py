"""CR1 full-builder confirmation; frozen opt-in binaries and paired receipts."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import resource
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits, STORAGE

CELLS = ('steady-64', 'steady-256', 'adversarial-64', 'bigrows-64')
FLAG = 'FABRIC_SPILL_WORKSPACE_EXPERIMENT'


def dump(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def process(argv, stem, env, limit):
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    began = time.monotonic()
    try:
        child = subprocess.run([str(x) for x in argv], env=env, capture_output=True,
                               text=True, timeout=limit)
    except subprocess.TimeoutExpired as error:
        stem.with_suffix('.stdout').write_bytes(error.stdout or b'')
        stem.with_suffix('.stderr').write_bytes(error.stderr or b'')
        dump(stem.with_suffix('.command.json'), {'argv': [str(x) for x in argv],
             'exit': None, 'stop_reason': 'timeout', 'limit_s': limit})
        raise
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    stem.with_suffix('.stdout').write_text(child.stdout)
    stem.with_suffix('.stderr').write_text(child.stderr)
    dump(stem.with_suffix('.command.json'), {'argv': [str(x) for x in argv],
         'experiment_flag': env.get(FLAG), 'exit': child.returncode,
         'whole_child_wall_s': time.monotonic() - began,
         'whole_child_user_s': after.ru_utime - before.ru_utime,
         'whole_child_system_s': after.ru_stime - before.ru_stime})
    if child.returncode:
        raise RuntimeError('child failed: ' + str(stem))
    return child.stdout


def equal(a, b):
    return all(a[key] == b[key] for key in (
        'input_sha256', 'journal_bytes', 'manifest', 'ordered_row_ledgers',
        'logical_row_counts', 'filter_check', 'pruning')) and not a['leftovers'] and not b['leftovers']


def phase_cpu(row):
    before = row['stat_before'].rsplit(')', 1)[1].split()
    after = row['stat_after'].rsplit(')', 1)[1].split()
    return sum(int(after[i]) - int(before[i]) for i in (11, 12)) / os.sysconf('SC_CLK_TCK')


def performance(pair):
    base, candidate = pair['baseline'], pair['candidate']
    if pair['counted']:
        return {'allocation_primary_10_percent': candidate['cumulative_requested_bytes'] * 10 <= base['cumulative_requested_bytes'] * 9,
                'peak_regression_5_percent': candidate['incremental_peak_heap_bytes'] * 100 <= base['incremental_peak_heap_bytes'] * 105,
                'peak_ceiling_80_mib': candidate['incremental_peak_heap_bytes'] <= 80 * 2**20}
    return {'phase_cpu_regression_5_percent': candidate['phase_cpu_s'] <= base['phase_cpu_s'] * 1.05,
            'phase_wall_regression_5_percent': candidate['build_seconds'] <= base['build_seconds'] * 1.05}


def main():
    require_limits()
    parser = argparse.ArgumentParser()
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=False)
    base = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True)
    if not base.is_relative_to(STORAGE / 'scratch') or shutil.disk_usage(base).free < 16 * 2**30:
        raise RuntimeError('owned data-drive scratch and 16 GiB free reserve required')
    work = base / 'builder-spill'
    work.mkdir()
    env = dict(os.environ)
    env.pop(FLAG, None)
    candidate_env = dict(env, **{FLAG: '1'})
    binaries, hashes = {}, {}
    try:
        # New tests also exercise actual 16/17/18 run boundaries through production merge.
        process(['cargo', 'test', '--offline', '--locked', '-p', 'fabric-server',
                 '--lib', 'segment::bounded'], args.out / 'candidate-controls', candidate_env, 600)
        for counted in (False, True):
            for candidate in (False, True):
                name = ('counted' if counted else 'plain') + ('-candidate' if candidate else '-baseline')
                command = ['cargo', 'build', '--offline', '--locked', '--release',
                           '-p', 'fabric-server', '--example', 'completion_builder']
                if counted:
                    command += ['--features', 'responsibility-alloc-probe']
                process(command, args.out / ('build-' + name), candidate_env if candidate else env, 600)
                binary = work / name
                shutil.copy2(Path(os.environ['CARGO_TARGET_DIR']) / 'release/examples/completion_builder', binary)
                binaries[name] = binary
                hashes[name] = sha(binary)
        dump(args.out / 'provenance.json', {'binary_sha256': hashes, 'scratch': str(work),
             'flag': FLAG, 'source_sha256': {str(p.relative_to(ROOT)): sha(p) for p in (
                 Path(__file__), ROOT / 'crates/fabric-server/src/segment/bounded.rs',
                 ROOT / 'crates/fabric-server/src/segment/bounded/spill.rs',
                 ROOT / 'crates/fabric-server/examples/completion_builder.rs',
                 ROOT / 'docs/experiments/benchmarks/catalog-builder-spill-protocol.md')},
             'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
             'cache': 'fresh process, warm shared fixture; no cache eviction',
             'affinity': sorted(os.sched_getaffinity(0))})
        # Gate rejection controls contain deliberate wrong output and escaped scratch.
        fixture_control = {key: 'fixed' for key in ('input_sha256', 'journal_bytes', 'manifest',
            'ordered_row_ledgers', 'logical_row_counts', 'filter_check', 'pruning')}
        fixture_control['leftovers'] = []
        if not equal(fixture_control, fixture_control):
            raise RuntimeError('unchanged gate control rejected')
        for key in ('manifest', 'ordered_row_ledgers', 'pruning', 'leftovers'):
            bad = dict(fixture_control, **{key: ['wrong']})
            if equal(fixture_control, bad):
                raise RuntimeError('negative gate control accepted: ' + key)
        measured = {'cumulative_requested_bytes': 100, 'incremental_peak_heap_bytes': 100,
                    'phase_cpu_s': 1, 'build_seconds': 1}
        candidate = dict(measured, cumulative_requested_bytes=85)
        for counted, field, bad_value in ((True, 'cumulative_requested_bytes', 91),
                                         (True, 'incremental_peak_heap_bytes', 106),
                                         (False, 'phase_cpu_s', 1.06),
                                         (False, 'build_seconds', 1.06)):
            pair = {'counted': counted, 'baseline': measured, 'candidate': candidate}
            if not all(performance(pair).values()):
                raise RuntimeError('unchanged performance control rejected')
            pair['candidate'] = dict(candidate, **{field: bad_value})
            if all(performance(pair).values()):
                raise RuntimeError('performance negative control accepted: ' + field)
        dump(args.out / 'gate-controls.json', {'unchanged_accepted': True, 'defects_rejected': True})
        pairs = []
        for cell in CELLS:
            shape, mib = cell.rsplit('-', 1)
            root = work / cell
            root.mkdir()
            fixture = json.loads(process([binaries['plain-baseline'], 'gen', root / 'fixture', shape, mib],
                                         args.out / (cell + '-fixture'), env, 300))
            old = ROOT / 'docs/experiments/benchmarks/data/lab-completion-run-01/memory' / ('builder-' + cell) / 'pair-1.json'
            historical = json.loads(old.read_text())['bounded']
            if fixture['input_sha256'] != historical['input_sha256']:
                raise RuntimeError('original frozen input changed')
            for counted in (False, True):
                build = 'counted' if counted else 'plain'
                for repetition in range(3):
                    rows = {}
                    order = ('baseline', 'candidate') if repetition % 2 == 0 else ('candidate', 'baseline')
                    for variant in order:
                        name = build + '-' + variant
                        stem = f'{cell}-{build}-{repetition+1}-{variant}'
                        row = json.loads(process([binaries[name], 'run', root / stem, fixture['input'], 'bounded'],
                                                 args.out / stem, env, 180))
                        if row['allocator_counted'] != counted or row['spill_workspace_experiment'] != (variant == 'candidate'):
                            raise RuntimeError('wrong frozen binary mode')
                        metrics = ('cumulative_requested_bytes', 'successful_alloc_realloc_calls',
                                   'incremental_peak_heap_bytes')
                        if counted:
                            if any(not isinstance(row[key], int) or row[key] <= 0 for key in metrics):
                                raise RuntimeError('counted allocator measurement absent')
                        elif any(row[key] is not None for key in metrics):
                            raise RuntimeError('plain allocator metrics must be unavailable')
                        if row['manifest'] != historical['manifest'] or row['input_sha256'] != fixture['input_sha256']:
                            raise RuntimeError('historical bounded output changed')
                        row['phase_cpu_s'] = phase_cpu(row)
                        if row['phase_cpu_s'] <= 0:
                            raise RuntimeError('phase CPU below process-jiffy measurement resolution')
                        rows[variant] = row
                    if not equal(rows['baseline'], rows['candidate']):
                        raise RuntimeError('exact builder differential failed')
                    pair = {'cell': cell, 'counted': counted, 'repetition': repetition+1, 'order': order, **rows}
                    pair['performance_guards'] = performance(pair)
                    dump(args.out / f'{cell}-{build}-{repetition+1}.json', pair)
                    pairs.append(pair)
                    for variant in order:
                        shutil.rmtree(root / f'{cell}-{build}-{repetition+1}-{variant}')
            shutil.rmtree(root)
        if any(sha(binary) != hashes[name] for name, binary in binaries.items()):
            raise RuntimeError('frozen binary changed')
        peaks = {cell: max(p['candidate']['incremental_peak_heap_bytes'] for p in pairs
                          if p['counted'] and p['cell'] == cell) for cell in ('steady-64', 'steady-256')}
        scaling = peaks['steady-256'] * 10 <= peaks['steady-64'] * 11
        nomination = scaling and all(all(pair['performance_guards'].values()) for pair in pairs)
        dump(args.out / 'result.json', {'pairs': pairs, 'exact_bounded_output': True,
             'nomination_guards_passed': nomination, 'scaling_within_10_percent': scaling,
             'performance_failure_is_collected_evidence': True,
             'scope': 'single-builder confirmation; historical C2 large-row failure unchanged'})
        archive = args.out / 'binaries'
        archive.mkdir()
        for name, binary in binaries.items():
            with binary.open('rb') as source, gzip.open(archive / (name + '.gz'), 'wb') as dest:
                shutil.copyfileobj(source, dest)
        shutil.rmtree(work)
        dump(args.out / 'cleanup.json', {'removed': True})
    except BaseException as error:
        dump(args.out / 'failure.json', {'error': repr(error), 'retained_scratch': str(work)})
        raise


if __name__ == '__main__':
    main()
