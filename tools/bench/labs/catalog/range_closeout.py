"""Preserve timeout evidence and finish unchanged manual tooling verification."""
import ast
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import time

import native_lifecycle_run as archive

common = archive.common
ROOT = common.ROOT
NAME = 'catalog-range-closeout-01'
PREVIOUS = 'catalog-range-evidence-checks-01'
UNIT = 'fabric-work-2c327df39b484691aa045de50e4d5e34'


def same_source(expected, actual):
    if expected != actual:
        raise RuntimeError('completed fast source differs')


def main():
    common.require_limits()
    ast.parse(Path(__file__).read_text())
    ast.parse((ROOT / 'tools/bench/labs/completion/run_job.py').read_text())
    base = ROOT / 'docs/experiments/benchmarks/data'
    out = base / NAME
    out.mkdir(exist_ok=False)
    work = Path(os.environ['FABRIC_SCRATCH_ROOT']) / NAME
    work.mkdir()
    receipt = {'state': 'running', 'commands': [], 'scratch': str(work),
               'historical_fast_job': PREVIOUS, 'preserved_timeout_unit': UNIT}
    common.dump(out / 'receipt.json', receipt)
    deadline = time.monotonic() + 50
    try:
        receipt['archive_controls_rejected'] = archive.archive_controls(work)
        same_source(b'unchanged', b'unchanged')
        try:
            same_source(b'unchanged', b'changed')
        except RuntimeError:
            receipt['source_change_control_rejected'] = True
        else:
            raise RuntimeError('source change control accepted')
        status = subprocess.run(['systemctl', '--user', 'is-active', UNIT + '.service'],
                                capture_output=True, text=True, timeout=5)
        if status.returncode not in (3, 4) or status.stdout.strip() not in ('inactive', 'failed', 'unknown'):
            raise RuntimeError('previous unit still active or not inspectable')
        held = Path('/run/media/kmosoti/data/FabricO11y/evidence') / UNIT
        if held.is_symlink() or not held.is_dir():
            raise RuntimeError('expected owned failure evidence unavailable')
        held_out = out / 'previous-failure'
        held_out.mkdir()
        preserved = archive.preserve(held, held_out, 128 * 1024)
        shutil.rmtree(held)
        receipt['previous_failure'] = {'path': str(held), 'unit_state': status.stdout.strip(),
            'exact_readback': preserved['exact_readback'],
            'decoded_file_bytes': preserved['decoded_file_bytes'], 'removed': not held.exists()}
        previous = base / 'lab-completion-run-01/coordinator' / PREVIOUS
        compared = {}
        with tarfile.open(previous / 'source.tar.gz', 'r:gz') as source:
            for member in source.getmembers():
                path = ROOT / member.name
                if not member.isfile() or not (path.suffix in ('.rs', '.c')
                        or path.name in ('Cargo.toml', 'Cargo.lock')
                        or member.name.startswith('tools/qualification/')):
                    continue
                same_source(source.extractfile(member).read(), path.read_bytes())
                compared[member.name] = common.sha(path)
        if not compared:
            raise RuntimeError('empty source comparison')
        receipt['unchanged_product_source_sha256'] = compared
        old_fast = base / 'lab-completion-run-01/recovery' / PREVIOUS
        if json.loads((old_fast / 'command.json').read_text())['exit'] != 0:
            raise RuntimeError('historical fast profile did not pass')
        shutil.copytree(old_fast, out / 'historical-fast-receipts')
        shutil.copyfile(ROOT / 'target/verification/receipts/docs.json', out / 'historical-docs.json')
        (out / 'final-documentation.diff').write_bytes(subprocess.check_output(
            ['git', 'diff', '--', 'docs'], cwd=ROOT))
        final_docs = out / 'final-documents'
        final_docs.mkdir()
        for name in ('docs/experiments/benchmarks/catalog-range-evidence-findings.md',
                     'docs/CURRENT.md', 'docs/research/range-evidence-and-work-ownership.md',
                     'docs/architecture/retained-history.md', 'docs/diagrams/read-catalog.mmd'):
            shutil.copyfile(ROOT / name, final_docs / (Path(name).name + '.txt'))
        command = [sys.executable, '-B', 'tools/bench/labs/completion/checks.py',
                   '--profile', 'documentation', '--id', NAME + '-docs']
        code = common.profile.run_child(command, dict(os.environ, FABRIC_SCRATCH_ROOT=str(work)),
            out / 'documentation.stdout', out / 'documentation.stderr', deadline, work, out)
        receipt['commands'].append({'argv': command, 'exit': code})
        if code:
            raise RuntimeError('manual documentation profile failed')
        if compared != {name: common.sha(ROOT / name) for name in compared}:
            raise RuntimeError('product source changed during closeout')
        receipt['state'] = 'passed'
        shutil.rmtree(work)
    except BaseException as error:
        receipt.update(state='failed', error=repr(error))
        if work.exists():
            receipt['failure_fixture'] = archive.preserve(work, out, 128 * 1024)
            shutil.rmtree(work)
        raise
    finally:
        import coupled_bun_preserve as bun
        target = Path(os.environ['TMPDIR']) / 'bun'
        if target.exists():
            stored = bun.BASE / bun.ARCHIVE
            if bun.sha(stored) != bun.ARCHIVE_SHA:
                raise RuntimeError('canonical Bun archive changed; retain copy')
            bun.compare(stored, bun.MEMBER, target, bun.PAYLOAD_SHA, bun.PAYLOAD_BYTES)
            bun.persist(out / 'bun-reference.json', {'archive': str(stored.relative_to(ROOT)),
                'archive_sha256': bun.ARCHIVE_SHA, 'member': bun.MEMBER,
                'decoded_sha256': bun.PAYLOAD_SHA, 'bytes': bun.PAYLOAD_BYTES,
                'exact_readback': True})
            target.unlink()
        receipt['bun_copy_removed'] = not target.exists()
        receipt['scratch_removed'] = not work.exists()
        allocated = sum(max(path.stat().st_size, path.stat().st_blocks * 512)
                        for path in out.rglob('*') if path.is_file())
        receipt['driver_evidence_allocated_bytes_before_receipt'] = allocated
        if allocated + 65536 > 512 * 1024:
            receipt.update(state='failed', error='driver evidence cap exceeded')
            common.dump(out / 'receipt.json', receipt)
            raise RuntimeError('driver evidence cap exceeded')
        common.dump(out / 'receipt.json', receipt)


if __name__ == '__main__':
    main()
