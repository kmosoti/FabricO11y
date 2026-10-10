"""Finite ordered-join progress counterexample, correction and verification."""
import fcntl
import gzip
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
from cross_system_readiness import compressed

common = archive.common
ROOT = common.ROOT
PROTOCOL = ROOT / 'docs/experiments/benchmarks/catalog-prefix-progress-protocol.md'
ALLOCATION = ROOT / 'docs/experiments/benchmarks/catalog-prefix-checks-allocation.md'
CLOSEOUT = ROOT / 'docs/experiments/benchmarks/catalog-prefix-documentation-closeout.md'
SEALER = 'crates/fabric-server/src/sealer.rs'


def main():
    phase = sys.argv[1]
    if phase not in ('baseline', 'candidate', 'checks', 'documentation'):
        raise ValueError('unknown registered phase')
    started = time.monotonic()
    common.require_limits()
    identity = 'catalog-prefix-progress-' + phase + '-01'
    verifying = phase in ('checks', 'documentation')
    seconds = 34 if phase == 'documentation' else 350 if phase == 'checks' else 120
    stage = 'verification' if verifying else 'recovery'
    stage_limit = 4040 if verifying else 7200
    base = ROOT / 'docs/experiments/benchmarks/data/lab-completion-run-01'
    with (ROOT / 'target/readiness-lab.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        prior = [json.loads(p.read_text()) for p in (base / 'coordinator').glob('*/receipt.json')]
        if any(r['state'] == 'running' for r in prior):
            raise RuntimeError('another coordinator workload is running')
        used = sum(r.get('elapsed_s', 0) for r in prior)
        frontier = sum(r.get('elapsed_s', 0) for r in prior if r['id'].startswith(('catalog-', 'frontier-')))
        stage_used = sum(r.get('elapsed_s', 0) for r in prior if r.get('stage') == stage)
        prep_used = sum(r.get('elapsed_s', 0) for r in prior if r.get('stage') == 'preparation')
        if prep_used > 3160 or min(86400 - used, 14400 - frontier, stage_limit - stage_used) < seconds:
            raise RuntimeError('phase does not fit prospective allocation')
        if shutil.disk_usage(os.environ['TMPDIR']).free < 16 * 1024**3:
            raise RuntimeError('disk reserve unavailable')
        before = coupled_admit.observe(192 * 1024, 0)
        out = base / 'coordinator' / identity
        out.mkdir()
        work = Path(os.environ['TMPDIR']) / identity
        work.mkdir()
        relative = next(s[3:] for s in Path('/proc/self/cgroup').read_text().splitlines() if s.startswith('0::'))
        group = Path('/sys/fs/cgroup') / relative.lstrip('/')
        record = dict(id=identity, lab='recovery', stage=stage, state='running', phase=phase,
            timeout_s=seconds, stage_limit_s=stage_limit, stage_prior_execution_s=stage_used,
            preparation_limit_s=3160, preparation_prior_execution_s=prep_used,
            frontier_prior_execution_s=frontier, prior_execution_s=used,
            cgroup=relative, scratch=str(work), before=before, commands=[],
            limits={n: (group / n).read_text().strip() for n in
                    ('memory.max', 'memory.high', 'memory.swap.max', 'cpu.max')})
        compressed(out / 'driver.py.gz', Path(__file__).read_bytes())
        compressed(out / 'protocol.txt.gz', PROTOCOL.read_bytes())
        compressed(out / 'allocation.txt.gz', ALLOCATION.read_bytes())
        if phase == 'documentation':
            compressed(out / 'closeout.txt.gz', CLOSEOUT.read_bytes())
            same_source((ROOT / SEALER).read_bytes(), gzip.decompress(
                (base / 'coordinator/catalog-prefix-progress-checks-01/sealer.rs.gz').read_bytes()))
        compressed(out / 'sealer.rs.gz', (ROOT / SEALER).read_bytes())
        record['source_sha256'] = {str(p.relative_to(ROOT)): common.sha(p) for p in
            (Path(__file__).resolve(), PROTOCOL, ALLOCATION, ROOT / SEALER,
             ROOT / 'tools/bench/labs/catalog/cross_system_readiness.py')}
        common.dump(out / 'receipt.json', record)
        commands = {
            'baseline': [(['cargo', 'test', '--offline', '--locked', '-p', 'fabric-server', '--lib',
                          'sealer::tests::completed_prefix_reclaims_while_sibling_is_blocked', '--', '--exact'], 101)],
            'candidate': [(['cargo', 'test', '--offline', '--locked', '-p', 'fabric-server', '--lib', 'sealer::tests'], 0),
                          (['cargo', 'test', '--offline', '--locked', '-p', 'fabric-server', '--test', 'history'], 0)],
            'checks': [([sys.executable, '-B', 'tools/bench/labs/completion/checks.py', '--profile', profile,
                         '--id', identity + '-' + profile], 0) for profile in ('fast', 'documentation')],
            'documentation': [([sys.executable, '-B', 'tools/bench/labs/completion/checks.py',
                '--profile', 'documentation', '--id', identity + '-documentation'], 0)],
        }[phase]
        code = 1
        try:
            if verifying:
                unit = ('fabric-work-99cb4c335d72407f9f84a4b1f50d545d' if phase == 'documentation'
                        else 'fabric-work-e924e62f84504e13bd441be97ddf6718')
                status = subprocess.run(['systemctl', '--user', 'is-active', unit + '.service'],
                    capture_output=True, text=True, timeout=5)
                if status.returncode not in (3, 4) or status.stdout.strip() not in ('inactive', 'failed', 'unknown'):
                    raise RuntimeError('interrupted unit not confirmed inactive')
                held = Path('/run/media/kmosoti/data/FabricO11y/evidence') / unit
                if held.is_symlink() or not held.is_dir():
                    raise RuntimeError('expected interrupted state unavailable')
                failure = out / 'interrupted-candidate'
                failure.mkdir()
                record['archive_controls_rejected'] = archive.archive_controls(work)
                record['interrupted_candidate'] = archive.preserve(held, failure,
                    (64 if phase == 'documentation' else 512) * 1024)
                shutil.rmtree(held)
                record['interrupted_scratch_removed'] = not held.exists()
            source_path = base / 'coordinator/catalog-range-evidence-checks-01/source.tar.gz'
            checked = {}
            with tarfile.open(source_path, 'r:gz') as source:
                for member in source.getmembers():
                    path = ROOT / member.name
                    if member.isfile() and member.name != SEALER and (path.suffix in ('.rs', '.c')
                            or path.name in ('Cargo.toml', 'Cargo.lock')
                            or member.name.startswith('tools/qualification/')):
                        same_source(source.extractfile(member).read(), path.read_bytes())
                        checked[member.name] = common.sha(path)
            if not checked:
                raise RuntimeError('empty source comparison')
            record['unchanged_product_source_sha256'] = checked
            record['reused_source_archive'] = str(source_path.relative_to(ROOT))
            record['reused_source_archive_sha256'] = common.sha(source_path)
            for index, (command, expected) in enumerate(commands):
                log = out / f'command-{index}.txt'
                tick = time.monotonic()
                environment = dict(os.environ, FABRIC_SCRATCH_ROOT=str(work))
                if verifying:
                    environment.pop('RUST_TEST_THREADS', None)
                else:
                    environment['RUST_TEST_THREADS'] = '1'
                record['test_threads'] = environment.get('RUST_TEST_THREADS', 'workspace default')
                with log.open('wb') as stream:
                    process = subprocess.Popen(command, cwd=ROOT, stdout=stream, stderr=subprocess.STDOUT,
                        start_new_session=True, env=environment)
                    try:
                        while process.poll() is None:
                            if time.monotonic() - started > seconds - (4 if phase == 'documentation' else 12):
                                raise TimeoutError('registered phase deadline')
                            if log.stat().st_size > 64 * 1024:
                                raise RuntimeError('command output exceeds reservation')
                            time.sleep(.05)
                    except BaseException:
                        try:
                            os.killpg(process.pid, signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                        process.wait()
                        raise
                    finally:
                        record['commands'].append(dict(argv=command, exit=process.returncode,
                            expected_exit=expected, elapsed_s=time.monotonic() - tick, log=log.name))
                if process.returncode != expected:
                    raise RuntimeError('unexpected command exit')
                if phase == 'baseline' and 'completed prefix was retained behind a blocked sibling' not in log.read_text():
                    raise RuntimeError('baseline failed for a different reason')
            if checked != {name: common.sha(ROOT / name) for name in checked}:
                raise RuntimeError('product source changed while running')
            if common.sha(ROOT / SEALER) != record['source_sha256'][SEALER]:
                raise RuntimeError('sealer source changed while running')
            record['retained_test_scratch'] = archive.preserve(work, out, 64 * 1024) if any(work.iterdir()) else None
            shutil.rmtree(work)
            owned = list(out.rglob('*'))
            if verifying:
                for profile in (('documentation',) if phase == 'documentation' else ('fast', 'documentation')):
                    owned.extend((base / 'recovery' / (identity + '-' + profile)).rglob('*'))
            allocated = sum(max(p.stat().st_size, p.stat().st_blocks * 512) for p in owned if p.is_file())
            record['allocated_bytes_before_final_receipt'] = allocated
            if allocated + 32768 > 192 * 1024:
                raise RuntimeError('phase evidence exceeds reservation')
            record['state'] = 'baseline-rejected' if phase == 'baseline' else 'passed'
            code = 0
        except BaseException as error:
            record.update(state='failed', error=repr(error))
            raise
        finally:
            # The manual profile may stage its existing Bun executable here.
            bun_path = Path(os.environ['TMPDIR']) / 'bun'
            if bun_path.exists():
                import coupled_bun_preserve as bun
                stored = bun.BASE / bun.ARCHIVE
                if bun.sha(stored) != bun.ARCHIVE_SHA:
                    raise RuntimeError('canonical Bun archive changed')
                bun.compare(stored, bun.MEMBER, bun_path, bun.PAYLOAD_SHA, bun.PAYLOAD_BYTES)
                record['bun_reference'] = dict(archive=str(stored.relative_to(ROOT)), sha256=bun.ARCHIVE_SHA,
                                               exact_readback=True)
                bun_path.unlink()
            record['scratch_removed'] = not work.exists()
            record['cgroup_final'] = {n: (group / n).read_text().strip() for n in
                ('memory.peak', 'memory.events', 'memory.swap.current', 'cpu.stat', 'io.stat')}
            record['exit'] = code
            try:
                record['after'] = coupled_admit.observe(0, 0)
            finally:
                record['elapsed_s'] = time.monotonic() - started
                common.dump(out / 'receipt.json', record)


if __name__ == '__main__':
    main()
