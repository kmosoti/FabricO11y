"""Finite ownership-transfer checks and exact cleanup, inside the launcher."""
import ast
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

import coupled_bun_preserve as bun
from resource_group import require_limits

ROOT = Path(__file__).resolve().parents[4]
ID = 'catalog-owned-append-checks-01'
OUT = ROOT / 'docs/experiments/benchmarks/data' / ID


def main():
    require_limits()
    OUT.mkdir(exist_ok=False)
    receipt = {'state': 'running', 'commands': []}
    target = Path(os.environ['TMPDIR']) / 'bun'
    try:
        for relative in ('tools/bench/labs/catalog/delivery_read.py',
                         'tools/bench/labs/catalog/owned_append_checks.py'):
            ast.parse((ROOT / relative).read_text(), filename=relative)
        receipt['python_syntax'] = 'parsed'
        commands = [
            ['rustfmt', '--edition', '2024', 'src/spindle/spool.rs', 'src/spindle/runtime.rs',
             'examples/coupled_overlap_node.rs'],
            ['cargo', 'test', '--offline', '--locked', '-p', 'fabric_o11y', '--lib',
             'spindle::spool::', '--', '--test-threads=1'],
            [sys.executable, '-B', 'tools/bench/labs/completion/checks.py', '--profile', 'fast', '--id', ID],
            [sys.executable, '-B', 'tools/bench/labs/completion/checks.py', '--profile', 'documentation', '--id', ID + '-docs'],
        ]
        env = dict(os.environ)
        for key in ('FABRIC_ACK_ADVANCE_EXPERIMENT', 'FABRIC_SPILL_WORKSPACE_EXPERIMENT',
                    'FABRIC_RUN_MIB_EXPERIMENT', 'FABRIC_BORROWED_LOG_EXPERIMENT'):
            env.pop(key, None)
        for index, argv in enumerate(commands):
            with (OUT / f'{index}.stdout').open('wb') as stdout, (OUT / f'{index}.stderr').open('wb') as stderr:
                result = subprocess.run(argv, cwd=ROOT, env=env, stdout=stdout, stderr=stderr)
            receipt['commands'].append({'argv': argv, 'exit': result.returncode})
            (OUT / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
            if result.returncode:
                raise RuntimeError('check failed: ' + ' '.join(argv))
        receipt['source_sha256'] = {p: bun.sha(ROOT / p) for p in (
            'src/spindle/spool.rs', 'src/spindle/runtime.rs', 'examples/coupled_overlap_node.rs')}
        receipt['state'] = 'passed'
    finally:
        if target.exists():
            archive = bun.BASE / bun.ARCHIVE
            if bun.sha(archive) != bun.ARCHIVE_SHA:
                raise RuntimeError('canonical Bun archive changed; keep scratch')
            bun.compare(archive, bun.MEMBER, target, bun.PAYLOAD_SHA, bun.PAYLOAD_BYTES)
            reference = {'source': str(target), 'archive': str(archive.relative_to(ROOT)),
                         'archive_sha256': bun.ARCHIVE_SHA, 'member': bun.MEMBER,
                         'decoded_sha256': bun.PAYLOAD_SHA, 'bytes': bun.PAYLOAD_BYTES,
                         'exact_readback': True}
            bun.persist(OUT / 'bun-reference.json', reference)
            target.unlink()
        receipt['bun_copy_removed'] = not target.exists()
        (OUT / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')


if __name__ == '__main__':
    main()
