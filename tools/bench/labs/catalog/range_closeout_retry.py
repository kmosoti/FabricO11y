"""Bounded, source-reusing repair of documentation snapshot packaging."""
import ast
import fcntl
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tarfile
import time

import coupled_admit
import native_lifecycle_run as archive
from range_closeout import same_source

common = archive.common
ROOT = common.ROOT
ID = 'catalog-range-closeout-02'
OLD = 'catalog-range-closeout-01'
UNIT = 'fabric-work-805846b579ac4601b7f704c85b0a741b'
PROTOCOL = ROOT / 'docs/experiments/benchmarks/catalog-range-closeout-retry-protocol.md'


def main():
    started = time.monotonic()
    common.require_limits()
    ast.parse(Path(__file__).read_text())
    base = ROOT / 'docs/experiments/benchmarks/data'
    coord = base / 'lab-completion-run-01/coordinator'
    with (ROOT / 'target/readiness-lab.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        prior = [json.loads(p.read_text()) for p in coord.glob('*/receipt.json')]
        if any(r['state'] == 'running' for r in prior):
            raise RuntimeError('coordinator work still running')
        total = sum(r.get('elapsed_s', 0) for r in prior)
        frontier = sum(r.get('elapsed_s', 0) for r in prior if r['id'].startswith(('catalog-', 'frontier-')))
        stage = sum(r.get('elapsed_s', 0) for r in prior if r.get('stage') == 'verification')
        if min(86400 - total, 14400 - frontier, 3660 - stage) < 35:
            raise RuntimeError('registered 35-second continuation does not fit')
        before = coupled_admit.observe(512 * 1024, 0)
        out = coord / ID
        out.mkdir()
        work = Path(os.environ['FABRIC_SCRATCH_ROOT']) / ID
        work.mkdir()
        relative = next(line[3:] for line in Path('/proc/self/cgroup').read_text().splitlines() if line.startswith('0::'))
        group = Path('/sys/fs/cgroup') / relative.lstrip('/')
        record = dict(id=ID, lab='coordinator', stage='verification', state='running',
            command=sys.argv, timeout_s=35, stage_limit_s=3660, stage_prior_execution_s=stage,
            prior_execution_s=total, frontier_limit_s=14400, frontier_prior_execution_s=frontier,
            cgroup=relative, scratch=str(work), started_unix_ns=time.time_ns(), before=before,
            source_sha256={str(p.relative_to(ROOT)): common.sha(p) for p in
                (Path(__file__).resolve(), PROTOCOL, Path(archive.__file__).resolve(),
                 ROOT / 'tools/bench/labs/catalog/range_closeout.py')},
            limits={name: (group / name).read_text().strip() for name in
                ('memory.max', 'memory.high', 'memory.swap.max', 'cpu.max')})
        shutil.copyfile(__file__, out / 'driver.py.txt')
        shutil.copyfile(PROTOCOL, out / 'protocol.txt')
        common.dump(out / 'receipt.json', record)
        code = 1

        def timeout(_signal, _frame):
            raise TimeoutError('registered 35-second continuation exhausted')

        signal.signal(signal.SIGALRM, timeout)
        signal.setitimer(signal.ITIMER_REAL, max(.1, 35 - (time.monotonic() - started)))
        try:
            record['archive_controls_rejected'] = archive.archive_controls(work)
            same_source(b'same', b'same')
            try:
                same_source(b'same', b'changed')
            except RuntimeError:
                record['source_change_control_rejected'] = True
            else:
                raise RuntimeError('changed source accepted')
            status = subprocess.run(['systemctl', '--user', 'is-active', UNIT + '.service'],
                                    capture_output=True, text=True, timeout=5)
            if status.returncode not in (3, 4) or status.stdout.strip() not in ('inactive', 'failed', 'unknown'):
                raise RuntimeError('previous unit not confirmed inactive')
            snapshots = base / OLD / 'final-documents'
            if snapshots.is_symlink() or not snapshots.is_dir():
                raise RuntimeError('expected snapshot directory absent')
            saved = out / 'document-snapshots'
            saved.mkdir()
            record['document_snapshots'] = archive.preserve(snapshots, saved, 256 * 1024)
            shutil.rmtree(snapshots)
            record['loose_document_snapshots_removed'] = not snapshots.exists()
            held = Path('/run/media/kmosoti/data/FabricO11y/evidence') / UNIT
            if held.is_symlink() or not held.is_dir():
                raise RuntimeError('expected previous failure tree absent')
            held_out = out / 'previous-failure'
            held_out.mkdir()
            record['previous_failure'] = archive.preserve(held, held_out, 128 * 1024)
            shutil.rmtree(held)
            record['previous_failure_removed'] = not held.exists()
            source_path = coord / 'catalog-range-evidence-checks-01/source.tar.gz'
            checked = {}
            with tarfile.open(source_path, 'r:gz') as source:
                for member in source.getmembers():
                    path = ROOT / member.name
                    if member.isfile() and (path.suffix in ('.rs', '.c')
                            or path.name in ('Cargo.toml', 'Cargo.lock')
                            or member.name.startswith('tools/qualification/')):
                        same_source(source.extractfile(member).read(), path.read_bytes())
                        checked[member.name] = common.sha(path)
            if not checked:
                raise RuntimeError('no product source compared')
            record['unchanged_product_source_sha256'] = checked
            record['reused_source_archive'] = str(source_path.relative_to(ROOT))
            record['reused_source_archive_sha256'] = common.sha(source_path)
            (out / 'final-documentation.diff').write_bytes(subprocess.check_output(['git', 'diff', '--', 'docs'], cwd=ROOT))
            shutil.copyfile(ROOT / 'docs/experiments/benchmarks/catalog-range-evidence-findings.md', out / 'final-findings.md.txt')
            command = [sys.executable, '-B', 'tools/bench/labs/completion/checks.py',
                       '--profile', 'documentation', '--id', ID + '-docs']
            result = common.profile.run_child(command, dict(os.environ, FABRIC_SCRATCH_ROOT=str(work)),
                out / 'documentation.stdout', out / 'documentation.stderr', started + 30, work, out)
            record['verification_command'] = command
            record['verification_exit'] = result
            if result:
                raise RuntimeError('documentation profile failed')
            if checked != {name: common.sha(ROOT / name) for name in checked}:
                raise RuntimeError('product source changed')
            shutil.rmtree(work)
            record['state'] = 'passed'
            code = 0
        except BaseException as error:
            record.update(state='failed', error=repr(error))
            if work.exists():
                record['failure_fixture'] = archive.preserve(work, out, 128 * 1024)
                shutil.rmtree(work)
            raise
        finally:
            signal.setitimer(signal.ITIMER_REAL, 0)
            import coupled_bun_preserve as bun
            target = Path(os.environ['TMPDIR']) / 'bun'
            if target.exists():
                stored = bun.BASE / bun.ARCHIVE
                if bun.sha(stored) != bun.ARCHIVE_SHA:
                    raise RuntimeError('Bun archive changed')
                bun.compare(stored, bun.MEMBER, target, bun.PAYLOAD_SHA, bun.PAYLOAD_BYTES)
                common.dump(out / 'bun-reference.json', {'archive': str(stored.relative_to(ROOT)),
                    'sha256': bun.ARCHIVE_SHA, 'member': bun.MEMBER, 'exact_readback': True})
                target.unlink()
            record['scratch_removed'] = not work.exists()
            record['bun_copy_removed'] = not target.exists()
            record['after'] = coupled_admit.observe(0, 0)
            record['cgroup_final'] = {name: (group / name).read_text().strip() for name in
                ('memory.peak', 'memory.events', 'memory.swap.current', 'cpu.stat', 'io.stat')}
            record['exit'] = code
            record['elapsed_s'] = time.monotonic() - started
            allocated = sum(max(p.stat().st_size, p.stat().st_blocks * 512) for p in out.rglob('*') if p.is_file())
            record['allocated_bytes_before_final_receipt'] = allocated
            if allocated + 65536 > 512 * 1024:
                record.update(state='failed', exit=1, error='evidence cap exceeded')
                common.dump(out / 'receipt.json', record)
                raise RuntimeError('evidence cap exceeded')
            common.dump(out / 'receipt.json', record)


if __name__ == '__main__':
    main()
