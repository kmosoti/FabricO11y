"""Registered serial kernel comparisons; execute only under resource_group."""
import ast
import json
import os
from pathlib import Path
import shutil
import statistics
import sys
import time

import coupled_overlap as common

ROOT = common.ROOT
ID = 'catalog-algorithm-round-01'
OUT = ROOT / 'docs/experiments/benchmarks/data' / ID
RUST = (
    'crates/fabric-core/src/delivery.rs', 'examples/group_plan_probe.rs',
    'crates/fabric-server/src/query.rs', 'crates/fabric-server/src/query/selection.rs',
    'src/spindle/runtime.rs', 'src/spindle/runtime_ownership_tests.rs',
)
SOURCES = (*RUST, 'src/spindle/spool.rs', 'Cargo.lock', 'Cargo.toml',
           'tools/bench/labs/catalog/algorithm_round.py')


def rows(path):
    result = []
    for line in path.read_text().splitlines():
        if '{' in line:
            try:
                value = json.loads(line[line.index('{'):])
            except json.JSONDecodeError:
                continue
            if isinstance(value, dict):
                result.append(value)
    return result


def index_rows(raw, keys, expected):
    keyed = {tuple(row[key] for key in keys): row for row in raw}
    if len(keyed) != len(raw) or set(keyed) != expected:
        raise RuntimeError('missing, duplicate or unexpected timing arms')
    return keyed


def summarize():
    # A completeness checker must reject a missing arm and a duplicate arm.
    for defective in ([], [{'id': 1}, {'id': 1}]):
        try:
            index_rows(defective, ['id'], {(1,)})
        except RuntimeError:
            pass
        else:
            raise RuntimeError('completeness negative control accepted')
    group = index_rows(rows(OUT / 'group.stdout'), ['size', 'shape', 'pair', 'variant'],
        {(n, s, p, v) for n in (1, 8, 32, 256, 2048) for s in ('hot', 'distinct')
         for p in (1, 2, 3) for v in ('legacy', 'indexed')})
    group_cells = []
    for n in (1, 8, 32, 256, 2048):
        for shape in ('hot', 'distinct'):
            ratios = [group[n, shape, p, 'indexed']['wall_ns'] /
                      group[n, shape, p, 'legacy']['wall_ns'] for p in (1, 2, 3)]
            for p in (1, 2, 3):
                for v in ('legacy', 'indexed'):
                    row = group[n, shape, p, v]
                    assert row['commits'] == row['required_commit_count'] == 131072
                    assert row['output_digest'] == row['expected_output_digest']
            guard = (max(ratios) <= .80 if n in (256, 2048) and shape == 'distinct'
                     else statistics.median(ratios) <= 1.10 if n in (1, 8) else None)
            group_cells.append({'size': n, 'shape': shape, 'paired_ratios': ratios,
                                'median_ratio': statistics.median(ratios), 'guard_met': guard})
    selection = index_rows(rows(OUT / 'selection.stdout'),
        ['body_bytes', 'order', 'capacity', 'pair', 'dense'],
        {(b, o, c, p, d) for b in (16, 1024)
         for o in ('ascending', 'descending', 'equaltime', 'mixed')
         for c in (21, 1001) for p in (1, 2, 3) for d in (False, True)})
    selection_cells = []
    for body in (16, 1024):
        for order in ('ascending', 'descending', 'equaltime', 'mixed'):
            for capacity in (21, 1001):
                ratios = []
                for pair in (1, 2, 3):
                    old = selection[body, order, capacity, pair, False]
                    new = selection[body, order, capacity, pair, True]
                    assert old['exact_rows_and_order'] and new['exact_rows_and_order']
                    assert old['accepted_offers'] == new['accepted_offers']
                    assert len(old['offer_and_sorted_wall_ns']) == len(new['offer_and_sorted_wall_ns']) == 5
                    ratios.append(statistics.median(new['offer_and_sorted_wall_ns']) /
                                  statistics.median(old['offer_and_sorted_wall_ns']))
                median = statistics.median(ratios)
                selection_cells.append({'body_bytes': body, 'order': order,
                    'capacity': capacity, 'paired_ratios': ratios, 'median_ratio': median,
                    'guard_met': median <= (.90 if order == 'descending' else 1.10)})
    encoding = index_rows(rows(OUT / 'encoding.stdout'), ['body_bytes', 'pair'],
                          {(b, p) for b in (128, 4000) for p in (1, 2, 3)})
    encoding_cells = []
    for body in (128, 4000):
        ratios = [encoding[body, p]['owned_ns'] / encoding[body, p]['borrowed_ns']
                  for p in (1, 2, 3)]
        assert all(encoding[body, p]['exact_bytes'] for p in (1, 2, 3))
        median = statistics.median(ratios)
        encoding_cells.append({'body_bytes': body, 'paired_ratios': ratios,
                               'median_ratio': median,
                               'guard_met': median <= (.95 if body == 4000 else 1.10)})
    return {'completeness_negative_controls_rejected': True,
            'group_raw_arms': len(group), 'selection_raw_arms': len(selection),
            'encoding_raw_pairs': len(encoding),
            'group': group_cells, 'selection': selection_cells, 'encoding': encoding_cells,
            'group_h1_met': all(c['guard_met'] is not False for c in group_cells),
            'selection_h1_met': all(c['guard_met'] for c in selection_cells),
            'encoding_h1_met': all(c['guard_met'] for c in encoding_cells),
            'selection_guard_interpretation': '1.10 also applied to all non-descending cells',
            'scope': 'finite kernel wall time; no service throughput, per-arm CPU or RSS claim'}


def main():
    common.require_limits()
    OUT.mkdir(exist_ok=False)
    work = Path(os.environ['FABRIC_SCRATCH_ROOT']) / 'algorithm-round'
    work.mkdir()
    deadline = time.monotonic() + 860
    receipt = {'state': 'running', 'commands': [], 'scratch': str(work)}
    env = dict(os.environ)
    for key in ('FABRIC_ACK_ADVANCE_EXPERIMENT', 'FABRIC_SPILL_WORKSPACE_EXPERIMENT',
                'FABRIC_RUN_MIB_EXPERIMENT', 'FABRIC_BORROWED_LOG_EXPERIMENT'):
        env.pop(key, None)
    def execute(label, argv):
        rc = common.profile.run_child(argv, env, OUT / (label + '.stdout'),
                                      OUT / (label + '.stderr'), deadline, work, OUT)
        receipt['commands'].append({'label': label, 'argv': argv, 'exit': rc})
        common.dump(OUT / 'receipt.json', receipt)
        if rc:
            raise RuntimeError('failed command: ' + label)
    try:
        ast.parse(Path(__file__).read_text(), filename=__file__)
        execute('format', ['rustfmt', '--edition', '2024', *RUST])
        frozen = {p: common.sha(ROOT / p) for p in SOURCES}
        for p in SOURCES:
            target = OUT / 'source' / p
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / p, target)
        shutil.copyfile(ROOT / 'docs/experiments/benchmarks/catalog-algorithm-round-protocol.md',
                        OUT / 'protocol.txt')
        receipt['source_sha256'] = frozen
        execute('toolchain', ['rustc', '--version', '--verbose'])
        execute('core-controls', ['cargo', 'test', '--offline', '--locked', '-p', 'fabric-core',
                                 'delivery::', '--', '--test-threads=1'])
        execute('ownership-controls', ['cargo', 'test', '--offline', '--locked', '-p',
            'fabric_o11y', '--lib', 'ownership_', '--', '--test-threads=1'])
        execute('selection-controls', ['cargo', 'test', '--offline', '--locked', '-p',
            'fabric-server', '--lib', 'query::selection::', '--', '--test-threads=1'])
        execute('group', ['cargo', 'run', '--offline', '--locked', '--release', '--example',
                          'group_plan_probe'])
        execute('selection', ['cargo', 'test', '--offline', '--locked', '--release', '-p',
            'fabric-server', '--lib', 'dense_selection_timing_probe', '--', '--ignored',
            '--nocapture', '--test-threads=1'])
        execute('encoding', ['cargo', 'test', '--offline', '--locked', '--release', '-p',
            'fabric_o11y', '--lib', 'ownership_encoding_finite_pairs', '--', '--ignored',
            '--nocapture', '--test-threads=1'])
        common.dump(OUT / 'summary.json', summarize())
        if frozen != {p: common.sha(ROOT / p) for p in SOURCES}:
            raise RuntimeError('source changed during comparison')
        allocated = sum(max(p.stat().st_size, p.stat().st_blocks * 512)
                        for p in OUT.rglob('*') if p.is_file())
        if allocated > 1024**2:
            raise RuntimeError('registered driver evidence cap exceeded')
        receipt.update(state='complete', allocated_evidence_bytes=allocated,
                       performance_disposition='see summary; misses are retained')
        shutil.rmtree(work)
    finally:
        receipt['scratch_removed'] = not work.exists()
        common.dump(OUT / 'receipt.json', receipt)


if __name__ == '__main__':
    main()
