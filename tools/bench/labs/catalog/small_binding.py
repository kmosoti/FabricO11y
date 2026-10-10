"""One registered compact-prefix comparison after the indexed small-case miss."""
import json
import os
from pathlib import Path
import shutil
import statistics
import time

import algorithm_round as first

common = first.common
ROOT = common.ROOT
OUT = ROOT / 'docs/experiments/benchmarks/data/catalog-small-binding-01'
SOURCES = ('crates/fabric-core/src/delivery.rs', 'examples/group_plan_probe.rs',
           'tools/bench/labs/catalog/small_binding.py',
           'tools/bench/labs/catalog/algorithm_round.py', 'Cargo.lock')


def main():
    common.require_limits()
    OUT.mkdir(exist_ok=False)
    work = Path(os.environ['FABRIC_SCRATCH_ROOT']) / 'small-binding'
    work.mkdir()
    deadline = time.monotonic() + 270
    receipt = {'state': 'running', 'commands': [], 'scratch': str(work)}
    def execute(label, argv):
        rc = common.profile.run_child(argv, dict(os.environ), OUT / (label + '.stdout'),
                                      OUT / (label + '.stderr'), deadline, work, OUT)
        receipt['commands'].append({'argv': argv, 'exit': rc, 'label': label})
        common.dump(OUT / 'receipt.json', receipt)
        if rc:
            raise RuntimeError('failed: ' + label)
    try:
        execute('format', ['rustfmt', '--edition', '2024', *SOURCES[:2]])
        frozen = {p: common.sha(ROOT / p) for p in SOURCES}
        receipt['source_sha256'] = frozen
        for p in SOURCES:
            target = OUT / 'source' / p
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / p, target)
        shutil.copyfile(ROOT / 'docs/experiments/benchmarks/catalog-small-binding-protocol.md',
                        OUT / 'protocol.txt')
        execute('controls', ['cargo', 'test', '--offline', '--locked', '-p', 'fabric-core',
                             'delivery::', '--', '--test-threads=1'])
        execute('group', ['cargo', 'run', '--offline', '--locked', '--release', '--example',
                          'group_plan_probe'])
        indexed = first.index_rows(first.rows(OUT / 'group.stdout'),
            ['size', 'shape', 'pair', 'variant'],
            {(n, s, p, v) for n in (1, 8, 32, 256, 2048) for s in ('hot', 'distinct')
             for p in (1, 2, 3) for v in ('legacy', 'indexed')})
        cells = []
        for size in (1, 8, 32, 256, 2048):
            for shape in ('hot', 'distinct'):
                ratios = []
                for pair in (1, 2, 3):
                    old = indexed[size, shape, pair, 'legacy']
                    new = indexed[size, shape, pair, 'indexed']
                    for row in (old, new):
                        assert row['required_commit_count'] == row['commits'] == 131072
                        assert row['output_digest'] == row['expected_output_digest']
                    ratios.append(new['wall_ns'] / old['wall_ns'])
                median = statistics.median(ratios)
                guard = (max(ratios) <= .80 if size in (256, 2048) and shape == 'distinct'
                         else median <= 1.10 if size in (1, 8) or (size == 32 and shape == 'distinct')
                         else None)
                cells.append({'size': size, 'shape': shape, 'paired_ratios': ratios,
                              'median_ratio': median, 'guard_met': guard})
        common.dump(OUT / 'summary.json', {
            'raw_arms': len(indexed), 'cells': cells,
            'h1_met': all(c['guard_met'] is not False for c in cells),
            'variant_label_indexed_means': 'inline first; compact secondary until32 total; then maps',
            'scope': 'new-binding decision kernel only; default durable facts; no service rate claim'})
        if frozen != {p: common.sha(ROOT / p) for p in SOURCES}:
            raise RuntimeError('comparison source drift')
        allocated = sum(max(p.stat().st_size, p.stat().st_blocks * 512)
                        for p in OUT.rglob('*') if p.is_file())
        if allocated > 512 * 1024:
            raise RuntimeError('512KiB driver evidence cap exceeded')
        receipt.update(state='complete', allocated_evidence_bytes=allocated)
        shutil.rmtree(work)
    finally:
        receipt['scratch_removed'] = not work.exists()
        common.dump(OUT / 'receipt.json', receipt)


if __name__ == '__main__':
    main()
