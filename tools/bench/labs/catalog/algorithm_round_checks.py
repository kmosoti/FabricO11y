"""Final registered fast/manual checks with exact temporary-runtime cleanup."""
import ast
import json
import os
from pathlib import Path
import sys
import time

import algorithm_round as round_one
import coupled_bun_preserve as bun

common = round_one.common
ROOT = common.ROOT
ID = 'catalog-algorithm-checks-01'
OUT = ROOT / 'docs/experiments/benchmarks/data' / ID


def main():
    common.require_limits()
    OUT.mkdir(exist_ok=False)
    target = Path(os.environ['TMPDIR']) / 'bun'
    work = Path(os.environ['FABRIC_SCRATCH_ROOT'])
    receipt = {'state': 'running', 'commands': []}
    common.dump(OUT / 'receipt.json', receipt)
    deadline = time.monotonic() + 860
    sources = (*round_one.SOURCES, 'tools/bench/labs/catalog/small_binding.py',
               'tools/bench/labs/catalog/algorithm_round_checks.py')
    frozen = {p: common.sha(ROOT / p) for p in sources}
    receipt['source_sha256'] = frozen
    env = dict(os.environ)
    for key in ('FABRIC_ACK_ADVANCE_EXPERIMENT', 'FABRIC_SPILL_WORKSPACE_EXPERIMENT',
                'FABRIC_RUN_MIB_EXPERIMENT', 'FABRIC_BORROWED_LOG_EXPERIMENT'):
        env.pop(key, None)
    try:
        for p in sources:
            if p.endswith('.py'):
                ast.parse((ROOT / p).read_text(), filename=p)
        receipt['python_syntax'] = 'parsed'
        for profile in ('fast', 'documentation'):
            argv = [sys.executable, '-B', 'tools/bench/labs/completion/checks.py',
                    '--profile', profile, '--id', ID + ('-docs' if profile == 'documentation' else '')]
            code = common.profile.run_child(argv, env, OUT / (profile + '.stdout'),
                                            OUT / (profile + '.stderr'), deadline, work, OUT)
            receipt['commands'].append({'argv': argv, 'exit': code})
            common.dump(OUT / 'receipt.json', receipt)
            if code:
                raise RuntimeError('verification failed: ' + profile)
        if frozen != {p: common.sha(ROOT / p) for p in sources}:
            raise RuntimeError('source changed during verification')
        receipt['state'] = 'passed'
    finally:
        if target.exists():
            archive = bun.BASE / bun.ARCHIVE
            if bun.sha(archive) != bun.ARCHIVE_SHA:
                raise RuntimeError('canonical Bun archive changed; retain scratch')
            bun.compare(archive, bun.MEMBER, target, bun.PAYLOAD_SHA, bun.PAYLOAD_BYTES)
            bun.persist(OUT / 'bun-reference.json', {
                'source': str(target), 'archive': str(archive.relative_to(ROOT)),
                'archive_sha256': bun.ARCHIVE_SHA, 'member': bun.MEMBER,
                'decoded_sha256': bun.PAYLOAD_SHA, 'bytes': bun.PAYLOAD_BYTES,
                'exact_readback': True})
            target.unlink()
        receipt['bun_copy_removed'] = not target.exists()
        common.dump(OUT / 'receipt.json', receipt)


if __name__ == '__main__':
    main()
