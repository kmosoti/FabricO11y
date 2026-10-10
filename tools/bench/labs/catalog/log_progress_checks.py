"""Registered final fast/manual checks for progress and owned log projection."""
import ast
import os
from pathlib import Path
import sys
import time

import coupled_bun_preserve as bun
import log_progress_round as experiment

common = experiment.common
ROOT = common.ROOT
ID = 'catalog-log-progress-checks-01'
OUT = ROOT / 'docs/experiments/benchmarks/data' / ID


def main():
    common.require_limits()
    OUT.mkdir(exist_ok=False)
    target = Path(os.environ['TMPDIR']) / 'bun'
    work = Path(os.environ['FABRIC_SCRATCH_ROOT'])
    sources = (*experiment.SOURCES,
               'tools/bench/labs/catalog/protocol_snapshot_dedup.py',
               'tools/bench/labs/catalog/log_progress_checks.py')
    receipt = {'state': 'running', 'commands': []}
    common.dump(OUT / 'receipt.json', receipt)
    deadline = time.monotonic() + 560
    env = dict(os.environ)
    for key in ('FABRIC_ACK_ADVANCE_EXPERIMENT', 'FABRIC_SPILL_WORKSPACE_EXPERIMENT',
                'FABRIC_RUN_MIB_EXPERIMENT', 'FABRIC_BORROWED_LOG_EXPERIMENT'):
        env.pop(key, None)
    def execute(label, argv):
        code = common.profile.run_child(argv, env, OUT / (label + '.stdout'),
            OUT / (label + '.stderr'), deadline, work, OUT)
        receipt['commands'].append({'argv': argv, 'exit': code})
        common.dump(OUT / 'receipt.json', receipt)
        if code:
            raise RuntimeError('verification failed: ' + label)
    try:
        for p in sources:
            if p.endswith('.py'):
                ast.parse((ROOT / p).read_text(), filename=p)
        receipt['python_syntax'] = 'parsed'
        execute('format', ['rustfmt', '--edition', '2024',
                          'crates/fabric-server/src/rows_ownership_tests.rs'])
        frozen = {p: common.sha(ROOT / p) for p in sources}
        receipt['source_sha256'] = frozen
        execute('projection-controls', ['cargo', 'test', '--offline', '--locked',
            '-p', 'fabric-server', '--lib', 'rows::ownership_tests::',
            '--', '--test-threads=1'])
        for profile in ('fast', 'documentation'):
            argv = [sys.executable, '-B', 'tools/bench/labs/completion/checks.py',
                    '--profile', profile,
                    '--id', ID + ('-docs' if profile == 'documentation' else '')]
            execute(profile, argv)
        if frozen != {p: common.sha(ROOT / p) for p in sources}:
            raise RuntimeError('source changed during verification')
        receipt['state'] = 'passed'
    except BaseException as error:
        receipt.update(state='failed', error=repr(error))
        raise
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
        allocated = sum(max(p.stat().st_size, p.stat().st_blocks * 512)
                        for p in OUT.rglob('*') if p.is_file())
        if allocated > 512 * 1024:
            raise RuntimeError('512KiB verification output cap exceeded')


if __name__ == '__main__':
    main()
