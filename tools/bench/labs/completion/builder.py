#!/usr/bin/env python3
"""Private C2 differential runner. No full acceptance claim; see builder-proposal.md."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits, STORAGE

CELLS = [(s, 64) for s in ('steady', 'outage', 'adversarial', 'bigrows')] + [('steady', n) for n in (16, 32, 128, 256)]
CEILING = 80 * 2**20


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def sha(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def semantics(row):
    manifest = copy.deepcopy(row['manifest'])
    for name, entry in manifest['files'].items():
        # Byte length/hash are physical representation. Index count is checked
        # against its own real Parquet groups, never compared as a logical count.
        manifest['files'][name] = {} if name == 'text_filter.bin' else {'rows': entry['rows']}
    return manifest


def pruning_equal(a, b):
    for table in ('logs', 'metrics'):
        for width in ('10', '60'):
            x = a['pruning'][table]['windows'][width]
            y = b['pruning'][table]['windows'][width]
            if x['matched_rows'] != y['matched_rows'] or x['windows'] != y['windows']:
                return False
    return True


def grade(a, b, shape):
    lg = b['pruning']['logs']['group_rows']
    # Registered bigrows exception requires actual early nonfinal groups.
    cap_closed = shape == 'bigrows' and any(n < 8192 for n in lg[:-1])
    files = ['metrics.parquet'] + ([] if cap_closed else ['logs.parquet'])
    filters = all(row['filter_check']['aligned'] and row['filter_check']['false_negatives'] == 0
                  and row['filter_check']['missing_control_rejected'] and row['filter_check']['cleared_control_rejected']
                  and row['filter_check']['logical_rows'] == row['logical_row_counts']['logs']
                  and row['filter_check']['physical_groups'] == row['pruning']['logs']['row_groups']
                  and row['filter_check']['physical_groups'] == row['filter_check']['declared_filter_rows']
                  for row in (a, b))
    return {
        'identical_input': a['input_sha256'] == b['input_sha256'] and a['journal_bytes'] == b['journal_bytes'],
        'ordered_logical_rows': a['ordered_row_ledgers'] == b['ordered_row_ledgers'] and a['logical_row_counts'] == b['logical_row_counts'],
        'manifest_semantics': semantics(a) == semantics(b),
        'applicable_byte_equality': all(a['manifest']['files'][name]['sha256'] == b['manifest']['files'][name]['sha256'] for name in files),
        'physical_filters_valid': filters,
        'pruning_equivalence': pruning_equal(a, b),
        'heap_ceiling': b['incremental_peak_heap_bytes'] <= CEILING,
        'no_leftovers': not a['leftovers'] and not b['leftovers'],
    }


def controls():
    windows = {str(w): {'matched_rows': 1, 'windows': [{'from_ns': 0, 'to_ns': w, 'matched_rows': 1, 'read_rows': 1}]} for w in (10, 60)}
    sample = {'manifest': {'records': 1, 'files': {name: {'rows': 1, 'sha256': 'x', 'bytes': 10} for name in ('logs.parquet', 'metrics.parquet', 'text_filter.bin')}},
              'input_sha256': 'x', 'journal_bytes': 10, 'ordered_row_ledgers': {'logs': 'x'}, 'logical_row_counts': {'logs': 1},
              'filter_check': {'aligned': True, 'false_negatives': 0, 'missing_control_rejected': True, 'cleared_control_rejected': True,
                               'logical_rows': 1, 'physical_groups': 1, 'declared_filter_rows': 1},
              'pruning': {table: {'windows': copy.deepcopy(windows), 'group_rows': [1], 'row_groups': 1} for table in ('logs', 'metrics')},
              'incremental_peak_heap_bytes': 10, 'leftovers': []}
    if not all(grade(sample, sample, 'steady').values()):
        raise RuntimeError('unchanged control rejected')
    reports = {}
    for defect in ('rows', 'count', 'filter', 'pruning', 'heap', 'leftover', 'bytes'):
        bad = copy.deepcopy(sample)
        if defect == 'rows': bad['ordered_row_ledgers']['logs'] = 'changed'
        elif defect == 'count': bad['manifest']['files']['logs.parquet']['rows'] = 2
        elif defect == 'filter': bad['filter_check']['declared_filter_rows'] = 2
        elif defect == 'pruning': bad['pruning']['logs']['windows']['10']['windows'][0]['read_rows'] = 2
        elif defect == 'heap': bad['incremental_peak_heap_bytes'] = CEILING + 1
        elif defect == 'leftover': bad['leftovers'] = ['.building-1']
        else: bad['manifest']['files']['logs.parquet']['sha256'] = 'changed'
        gates = grade(sample, bad, 'steady')
        if all(gates.values()): raise RuntimeError('negative control accepted: ' + defect)
        reports[defect] = gates
    reports['growth'] = {'within_10_percent': 111 * 10 <= 100 * 11}
    if reports['growth']['within_10_percent']: raise RuntimeError('growth defect accepted')
    return reports


def footprint(root):
    size = 0
    for path in root.rglob('*'):
        try:
            if path.is_file(): size += path.stat().st_size
        except FileNotFoundError:
            pass  # live spill rename/reclaim
    return size


def process(command, destination, scratch, seconds):
    began = time.monotonic()
    with destination.with_suffix('.stdout.json').open('wb') as out, destination.with_suffix('.stderr.txt').open('wb') as err:
        child = subprocess.Popen(command, stdout=out, stderr=err)
        reason = None
        try:
            while child.poll() is None:
                if time.monotonic() - began > seconds: reason = 'process_deadline'
                elif footprint(scratch) > 4 * 2**30: reason = 'scratch_limit'
                elif shutil.disk_usage(scratch).free < 4 * 2**30: reason = 'free_space_reserve'
                if reason: child.kill(); break
                time.sleep(0.2)
            status = child.wait()
        finally:
            if child.poll() is None: child.kill(); child.wait()
    write(destination.with_suffix('.command.json'), {'argv': command, 'exit': status, 'stop_reason': reason, 'process_wall_s': time.monotonic()-began})
    if status or reason: raise RuntimeError(f'{destination.name}: exit={status}, reason={reason}')
    row = json.loads(destination.with_suffix('.stdout.json').read_text())
    if 'stat_before' in row:
        # Fields after the final ')' begin at proc stat field 3 (state).
        before = row['stat_before'].rsplit(')', 1)[1].split()
        after = row['stat_after'].rsplit(')', 1)[1].split()
        ticks = os.sysconf('SC_CLK_TCK')
        row['build_cpu_user_s'] = (int(after[11])-int(before[11]))/ticks
        row['build_cpu_system_s'] = (int(after[12])-int(before[12]))/ticks
        row['cpu_tick_hz'] = ticks
        write(destination.with_suffix('.readback.json'), row)
    return row


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--out', type=Path, required=True)
    ap.add_argument('--cell', choices=[f'{s}-{n}' for s, n in CELLS], required=False)
    ap.add_argument('--controls', action='store_true')
    ap.add_argument('--binary', type=Path)
    ap.add_argument('--baseline', type=Path, help='steady-64 result.json from same frozen fixture/source/binary')
    ap.add_argument('--process-seconds', type=int, default=120)
    args = ap.parse_args()
    require_limits()
    if args.out.exists(): ap.error('fresh output required')
    if not 0 < args.process_seconds <= 300: ap.error('finite process limit 1..300 required')
    args.out.mkdir(parents=True)
    write(args.out/'controls.json', controls())
    if args.controls: return
    if not args.cell: ap.error('--cell required unless --controls')
    scratch_base = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True)
    if not scratch_base.is_relative_to(STORAGE/'scratch'): raise RuntimeError('data-drive scratch required')
    work = scratch_base / ('c2-' + args.cell)
    work.mkdir(exist_ok=False)
    binary = (args.binary or Path(os.environ['CARGO_TARGET_DIR'])/'release/examples/completion_builder').resolve(strict=True)
    frozen = {str(p.relative_to(ROOT)): sha(p) for p in (Path(__file__), Path(__file__).with_name('builder-proposal.md'), ROOT/'crates/fabric-server/examples/completion_builder.rs')}
    frozen['binary'] = sha(binary)
    write(args.out/'provenance.json', {'hashes': frozen, 'scratch': str(work), 'command': sys.argv, 'affinity': sorted(os.sched_getaffinity(0)),
        'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'],cwd=ROOT,text=True).strip()})
    shape, mib = args.cell.rsplit('-', 1)
    pairs = []
    try:
        fixture = process([str(binary),'gen',str(work/'fixture'),shape,mib],args.out/'fixture',work,args.process_seconds)
        for repetition in range(3):
            order = ('reference', 'bounded') if repetition % 2 == 0 else ('bounded', 'reference')
            rows = {}
            for builder in order:
                stem = f'{repetition+1}-{builder}'
                rows[builder] = process([str(binary),'run',str(work/stem),fixture['input'],builder],args.out/stem,work,args.process_seconds)
            gates = grade(rows['reference'],rows['bounded'],shape)
            pair = {'repetition': repetition+1, 'order': order, 'gates': gates, **rows}
            write(args.out/f'pair-{repetition+1}.json',pair)
            pairs.append(pair)
            if not all(gates.values()): raise RuntimeError('implemented gate failed: '+json.dumps(gates))
            for builder in order: shutil.rmtree(work/f'{repetition+1}-{builder}')
        deterministic = all(len({json.dumps(p[b]['manifest']['files'],sort_keys=True) for p in pairs})==1 for b in ('reference','bounded'))
        scale = None
        if args.cell == 'steady-256':
            if not args.baseline: raise RuntimeError('steady-256 requires same-campaign steady-64 baseline')
            prior = json.loads(args.baseline.read_text())
            if prior['hashes']!=frozen or prior['cell']!='steady-64' or not prior['implemented_gates_passed']: raise RuntimeError('baseline provenance/gates mismatch')
            scale = max(p['bounded']['incremental_peak_heap_bytes'] for p in pairs)*10 <= prior['worst_candidate_heap_bytes']*11
        result = {'cell':args.cell,'hashes':frozen,'determinism':deterministic,'scale_within_10_percent':scale,
            'worst_candidate_heap_bytes':max(p['bounded']['incremental_peak_heap_bytes'] for p in pairs),
            'implemented_gates_passed':deterministic and scale is not False,
            'full_bs_acceptance':False,'measurement_complete':False,'missing_metrics':['exact cumulative spill bytes'], 'pairs':len(pairs)}
        write(args.out/'result.json',result)
        if not result['implemented_gates_passed']: raise RuntimeError('determinism/scale failed')
        if sha(binary)!=frozen['binary']: raise RuntimeError('binary changed')
        size=footprint(work)
        shutil.rmtree(work)
        write(args.out/'cleanup.json',{'removed':True,'logical_bytes_before_cleanup':size})
    except BaseException as error:
        write(args.out/'failure.json',{'error':repr(error),'retained_scratch':str(work),'logical_bytes':footprint(work)})
        raise


if __name__ == '__main__':
    main()
