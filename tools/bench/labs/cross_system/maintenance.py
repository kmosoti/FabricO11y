"""Charge the recorded pre-admission refusal and remove only empty owned scratch."""
import json
from pathlib import Path
import shutil
import time

from source_fetch import require_limits

BASE = Path('docs/experiments/benchmarks/data/cross-system-run-01')
RECEIPTS = Path('target/resource-containment/runs')
DATA = Path('/run/media/kmosoti/data/FabricO11y')


def main():
    require_limits()
    imported = BASE / 'coordinator/sources-repin-01'
    outer = json.loads((RECEIPTS / 'fabric-work-31b4e3c4d5d24120bd8ef274d829df15.json').read_text())
    if not imported.exists():
        imported.mkdir()
        row = {'id': 'sources-repin-01', 'lab': 'source', 'state': 'failed', 'exit': outer['exit'],
               'elapsed_s': outer['elapsed_s'], 'argv': outer['command'],
               'origin': 'reconciled original launcher receipt; admission refused before child launch',
               'scratch_removed': outer['temporary_removed'], 'outer_receipt': outer}
        (imported / 'receipt.json').write_text(json.dumps(row, indent=2) + '\n')
    results = []
    for path in RECEIPTS.glob('*.json'):
        row = json.loads(path.read_text())
        command = row['command']
        if 'tools/bench/labs/cross_system/run_job.py' not in command or '--id' not in command:
            continue
        identity = command[command.index('--id') + 1]
        if not (BASE / 'coordinator' / identity / 'receipt.json').exists():
            continue
        retained = row.get('retained_failure_evidence')
        if not retained:
            continue
        root = Path(retained)
        if not root.exists():
            continue
        if root.parent != DATA / 'evidence' or root.is_symlink():
            raise RuntimeError('unexpected retained ownership')
        entries = list(root.rglob('*'))
        if any(not entry.is_dir() or entry.is_symlink() for entry in entries):
            results.append({'job': identity, 'path': str(root), 'removed': False,
                            'reason': 'nonempty failure evidence remains preserved'})
            continue
        shutil.rmtree(root)
        results.append({'job': identity, 'path': str(root), 'removed': not root.exists(),
                        'regular_files': 0, 'reason': 'only empty owned directories remained'})
    out = BASE / 'coordinator' / ('cleanup-' + str(time.time_ns()) + '.json')
    out.write_text(json.dumps({'imported_refusal_seconds': outer['elapsed_s'], 'cleanup': results}, indent=2) + '\n')
    print(out)


if __name__ == '__main__':
    main()
