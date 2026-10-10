"""Final registered verification for native lifecycle and startup recovery."""
import ast
import os
from pathlib import Path
import shutil
import sys
import time

import coupled_bun_preserve as bun
import native_lifecycle_run as experiment

common = experiment.common
ROOT = common.ROOT
ID = 'catalog-native-lifecycle-checks-01'
OUT = ROOT / 'docs/experiments/benchmarks/data' / ID


def main():
    common.require_limits()
    OUT.mkdir(exist_ok=False)
    work = Path(os.environ['FABRIC_SCRATCH_ROOT'])
    target = Path(os.environ['TMPDIR']) / 'bun'
    sources = (*experiment.SOURCES, 'crates/fabric-server/tests/native_lifecycle.rs',
               'tools/bench/labs/catalog/protocol_snapshot_dedup_round2.py',
               'tools/bench/labs/catalog/protocol_snapshot_dedup_round3.py',
               'tools/bench/labs/catalog/native_lifecycle_checks.py')
    receipt = {'state': 'running', 'commands': []}
    common.dump(OUT / 'receipt.json', receipt)
    env = dict(os.environ)
    for key in ('FABRIC_ACK_ADVANCE_EXPERIMENT', 'FABRIC_SPILL_WORKSPACE_EXPERIMENT',
                'FABRIC_RUN_MIB_EXPERIMENT', 'FABRIC_BORROWED_LOG_EXPERIMENT',
                'FABRIC_NATIVE_EVIDENCE'):
        env.pop(key, None)
    deadline = time.monotonic() + 560
    try:
        for path in sources:
            if path.endswith('.py'):
                ast.parse((ROOT / path).read_text(), filename=path)
        receipt['python_syntax'] = 'parsed'
        argv = ['rustfmt', '--edition', '2024', 'crates/fabric-server/tests/native_lifecycle.rs']
        code = common.profile.run_child(argv, env, OUT / 'format.stdout',
            OUT / 'format.stderr', deadline, work, OUT)
        receipt['commands'].append({'argv': argv, 'exit': code})
        if code:
            raise RuntimeError('final formatting failed')
        frozen = {path: common.sha(ROOT / path) for path in sources}
        receipt['source_sha256'] = frozen
        for path in ('crates/fabric-server/src/lib.rs', 'crates/fabric-server/tests/startup.rs',
                     'crates/fabric-server/tests/native_lifecycle.rs'):
            target_source = OUT / 'source' / path
            target_source.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / path, target_source)
        for profile in ('fast', 'documentation'):
            argv = [sys.executable, '-B', 'tools/bench/labs/completion/checks.py',
                    '--profile', profile,
                    '--id', ID + ('-docs' if profile == 'documentation' else '')]
            code = common.profile.run_child(argv, env, OUT / (profile + '.stdout'),
                OUT / (profile + '.stderr'), deadline, work, OUT)
            receipt['commands'].append({'argv': argv, 'exit': code})
            common.dump(OUT / 'receipt.json', receipt)
            if code:
                raise RuntimeError('verification failed: ' + profile)
        if frozen != {path: common.sha(ROOT / path) for path in sources}:
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
        allocated = sum(max(path.stat().st_size, path.stat().st_blocks * 512)
                        for path in OUT.rglob('*') if path.is_file())
        if allocated > 512 * 1024:
            raise RuntimeError('512KiB verification output cap exceeded')


if __name__ == '__main__':
    main()
