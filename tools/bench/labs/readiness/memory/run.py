#!/usr/bin/env python3
"""Coordinator-only private diagnostic runner; no qualification claim."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
from types import SimpleNamespace
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[5]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits
SEED = 0xA11FA001
CEILING = 80 * 1024**2


def dump(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as f:
        for chunk in iter(lambda: f.read(1024**2), b''):
            h.update(chunk)
    return h.hexdigest()


def metadata(value):
    m = copy.deepcopy(value['manifest'])
    m['files'] = {name: {'rows': f['rows']} for name, f in m['files'].items()}
    return m


def grade(a, b, baseline_heap=None):
    applicable_files = ['metrics.parquet'] + ([] if b['shape'] == 'bigrows' else ['logs.parquet'])
    return {
        'same_fixture': a['input_sha256'] == b['input_sha256'] and a['journal_bytes'] == b['journal_bytes'],
        'ordered_rows_and_custody': a['ordered_row_ledgers'] == b['ordered_row_ledgers'],
        'manifest_metadata': metadata(a) == metadata(b),
        'applicable_file_bytes': all(a['manifest']['files'][n]['sha256'] == b['manifest']['files'][n]['sha256'] for n in applicable_files),
        'heap_ceiling': b['incremental_peak_heap_bytes'] <= CEILING,
        'no_leftovers': not b['building_leftovers'] and not b['run_leftovers'],
        'scale': baseline_heap is None or b['incremental_peak_heap_bytes'] <= baseline_heap * 1.10,
    }


def controls():
    sample = {'shape': 'steady', 'input_sha256': 'input', 'journal_bytes': 1,
              'ordered_row_ledgers': {'logs': 'rows', 'metrics': 'm', 'spans': 's', 'gaps': 'g', 'batches': 'b'},
              'manifest': {'records': 2, 'files': {n: {'sha256': 'hash', 'rows': 2, 'bytes': 1} for n in ['logs.parquet', 'metrics.parquet']}},
              'incremental_peak_heap_bytes': 100, 'building_leftovers': [], 'run_leftovers': []}
    assert all(grade(sample, sample, 100).values())
    mutations = {}
    for name in ['altered_row', 'dropped_row', 'header', 'heap', 'leftover', 'growth']:
        bad = copy.deepcopy(sample)
        if name == 'altered_row': bad['ordered_row_ledgers']['logs'] = 'changed-row-digest'
        if name == 'dropped_row':
            bad['ordered_row_ledgers']['logs'] = hashlib.sha256(b'').hexdigest()
            bad['manifest']['files']['logs.parquet']['rows'] = 1
        if name == 'header': bad['manifest']['records'] = 1
        if name == 'heap': bad['incremental_peak_heap_bytes'] = CEILING + 1
        if name == 'leftover': bad['run_leftovers'] = ['.run-left']
        if name == 'growth': bad['incremental_peak_heap_bytes'] = 111
        gates = grade(sample, bad, 100)
        assert not all(gates.values()), name
        mutations[name] = {'rejected': True, 'gates': gates}
    # Ledger framing must distinguish missing/changed rows in real byte streams.
    def ledger(rows):
        h = hashlib.sha256()
        for row in rows: h.update(len(row).to_bytes(8, 'little')); h.update(row)
        return h.hexdigest()
    assert ledger([b'a', b'b']) != ledger([b'a', b'X'])
    assert ledger([b'a', b'b']) != ledger([b'a'])
    # Deterministic rename/delete counterexample: an enumerated old run path
    # disappears before stat. Only that race is tolerated; access errors surface.
    def vanished():
        raise FileNotFoundError(".run-old renamed before stat")
    survivor = SimpleNamespace(stat=lambda: SimpleNamespace(st_mode=stat.S_IFREG, st_size=7))
    disappearing = SimpleNamespace(stat=vanished)
    assert inventory([disappearing, survivor]) == 7
    def forbidden():
        raise PermissionError("inventory denied")
    try:
        inventory([SimpleNamespace(stat=forbidden)])
    except PermissionError:
        pass
    else:
        raise AssertionError("inventory swallowed PermissionError")
    return {'unchanged_accepted': True, 'mutations': mutations,
            'ledger_changed_and_dropped_rejected': True,
            'renamed_run_inventory_race_tolerated': True,
            'inventory_permission_error_propagated': True}


def inventory(paths):
    total = 0
    for path in paths:
        try:
            entry = path.stat()
        except FileNotFoundError:
            # Sealer renames/deletes private runs while the watchdog samples.
            continue
        if stat.S_ISREG(entry.st_mode):
            total += entry.st_size
    return total


def size(root):
    def onerror(error):
        if not isinstance(error, FileNotFoundError):
            raise error
    paths = (Path(directory) / name
             for directory, _, files in os.walk(root, onerror=onerror)
             for name in files)
    return inventory(paths)


def trial(binary, state, evidence, shape, mib, builder, deadline):
    argv = [str(binary), str(state), shape, str(mib), str(SEED), builder]
    dump(evidence / (builder + '-command.json'), {'argv': argv})
    started = time.monotonic()
    with (evidence / (builder + '-stdout.json')).open('wb') as out, (evidence / (builder + '-stderr.txt')).open('wb') as err:
        child = subprocess.Popen(argv, stdout=out, stderr=err)
        try:
            while True:
                pid, status, usage = os.wait4(child.pid, os.WNOHANG)
                if pid:
                    child.returncode = os.waitstatus_to_exitcode(status)
                    break
                if time.monotonic() - started > 300 or time.monotonic() > deadline: raise RuntimeError('trial/campaign deadline')
                if size(state.parent) > 4 * 1024**3: raise RuntimeError('4GiB scratch ceiling')
                if shutil.disk_usage(state.parent).free < 4 * 1024**3: raise RuntimeError('4GiB free-space reserve')
                time.sleep(0.1)
        except BaseException:
            child.kill(); child.wait(); raise
    if child.returncode: raise RuntimeError(f'{builder} exit {child.returncode}')
    row = json.loads((evidence / (builder + '-stdout.json')).read_text())
    row.update(exit_code=child.returncode, user_seconds=usage.ru_utime,
               system_seconds=usage.ru_stime, whole_process_peak_rss_kib=usage.ru_maxrss,
               process_wall_seconds=time.monotonic() - started)
    return row


def main():
    require_limits()
    ap = argparse.ArgumentParser()
    ap.add_argument('--phase', required=True, choices=['controls', 'shapes', 'scale'])
    ap.add_argument('--destination', type=Path, required=True)
    ap.add_argument('--baseline', type=Path)
    args = ap.parse_args()
    if args.destination.exists(): ap.error('destination must not exist')
    args.destination.mkdir(parents=True)
    dump(args.destination / 'negative-controls.json', controls())
    if args.phase == 'controls':
        dump(args.destination / 'complete.json', {'exit_code': 0, 'controls_only': True}); return
    binary = Path(os.environ['CARGO_TARGET_DIR']) / 'release/examples/readiness_memory_lab'
    scratch = Path(os.environ.get('FABRIC_LAB_SCRATCH', os.environ['FABRIC_SCRATCH_ROOT']))
    storage = Path('/run/media/kmosoti/data/FabricO11y').resolve()
    if not scratch.resolve().is_relative_to(storage): raise RuntimeError('scratch is outside data drive')
    work = scratch / ('memory-' + args.phase)
    if work.exists(): raise RuntimeError('owned scratch already exists')
    work.mkdir()
    frozen = {'binary_sha256': digest(binary), 'protocol_sha256': digest(Path(__file__).with_name('protocol.md')),
              'runner_sha256': digest(Path(__file__)), 'source_sha256': digest(ROOT / 'crates/fabric-server/examples/readiness_memory_lab.rs'),
              'scratch': str(work), 'seed': SEED, 'cpu_affinity': sorted(os.sched_getaffinity(0)),
              'cgroup_metrics': None, 'cgroup_metrics_reason': 'coordinator records launcher observations',
              'offered_committed_acked': None, 'custody_counter_reason': 'offline Segment builder screen; no network ingest'}
    dump(args.destination / 'metadata.json', frozen)
    shutil.copyfile(ROOT / 'crates/fabric-server/examples/readiness_memory_lab.rs', args.destination / 'source.rs')
    shutil.copyfile(Path(__file__).with_name('protocol.md'), args.destination / 'protocol.txt')
    shutil.copyfile(Path(__file__), args.destination / 'runner.py')
    (args.destination / 'working-tree.diff').write_bytes(subprocess.check_output(['git', 'diff', 'HEAD'], cwd=ROOT))
    cells = [(s, 64) for s in ['steady', 'outage', 'adversarial', 'bigrows']] if args.phase == 'shapes' else [('steady', 256)]
    baseline_heap = None
    if args.phase == 'scale':
        if not args.baseline: ap.error('scale requires baseline')
        baseline = json.loads(args.baseline.read_text())
        prior = json.loads((args.baseline.parent.parent / 'metadata.json').read_text())
        for key in ['binary_sha256', 'protocol_sha256', 'runner_sha256', 'source_sha256']:
            if prior[key] != frozen[key]: raise RuntimeError('baseline hash mismatch: ' + key)
        baseline_heap = baseline['bounded']['incremental_peak_heap_bytes']
    pairs = []
    deadline = time.monotonic() + (900 if args.phase == 'scale' else 1500)
    try:
        for index, (shape, mib) in enumerate(cells):
            dest = args.destination / f'{shape}-{mib}'; dest.mkdir()
            rows = {}
            order = ['reference', 'bounded'] if index % 2 == 0 else ['bounded', 'reference']
            for builder in order:
                state = work / f'{shape}-{mib}-{builder}'
                rows[builder] = trial(binary, state, dest, shape, mib, builder, deadline)
            gates = grade(rows['reference'], rows['bounded'], baseline_heap)
            pair = dict(rows, shape=shape, mib=mib, order=order, gates=gates)
            dump(dest / 'pair.json', pair)
            if not all(gates.values()): raise RuntimeError(f'gate failure: {shape}: {gates}')
            for builder in order: shutil.rmtree(work / f'{shape}-{mib}-{builder}')
            dump(dest / 'cleanup.json', {'removed': True, 'scratch_remaining_bytes': size(work)})
            pairs.append({'shape': shape, 'mib': mib, 'gates': gates})
            print(json.dumps(pairs[-1]), flush=True)
        shutil.rmtree(work)
        dump(args.destination / 'cleanup.json', {'removed': not work.exists()})
        dump(args.destination / 'complete.json', {'exit_code': 0, 'pairs': pairs, 'full_bs_acceptance': False})
    except BaseException as error:
        dump(args.destination / 'failure.json', {'error': str(error), 'retained_scratch': str(work)})
        raise


if __name__ == '__main__': main()
