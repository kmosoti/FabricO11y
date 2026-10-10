"""Registered, source-reusing restart of the cross-system research queue."""
import fcntl
import gzip
import hashlib
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
ID = 'catalog-cross-system-readiness-01'
PIN = 'ff97ec42cdef94651a6985eb47e8f9d07e4608cd'
PROTOCOL = ROOT / 'docs/experiments/benchmarks/catalog-cross-system-readiness-protocol.md'
PRECHECKS = ('fabric-work-806ce58a0f7244f4a193d1aceab1a162',
             'fabric-work-d9bb5a6272ee4dfe9496c00f4ce398af')


def compressed(path, payload):
    path.write_bytes(gzip.compress(payload, mtime=0))
    same_source(payload, gzip.decompress(path.read_bytes()))


def main():
    started = time.monotonic()
    common.require_limits()
    coord = ROOT / 'docs/experiments/benchmarks/data/lab-completion-run-01/coordinator'
    with (ROOT / 'target/readiness-lab.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        prior = [json.loads(p.read_text()) for p in coord.glob('*/receipt.json')]
        if any(r['state'] == 'running' for r in prior):
            raise RuntimeError('coordinator job already running')
        prechecks = [json.loads((ROOT / 'target/resource-containment/runs' / (unit + '.json')).read_text())
                     for unit in PRECHECKS]
        precheck_s = sum(r['elapsed_s'] for r in prechecks)
        total = sum(r.get('elapsed_s', 0) for r in prior)
        frontier = sum(r.get('elapsed_s', 0) for r in prior if r['id'].startswith(('catalog-', 'frontier-')))
        stage = sum(r.get('elapsed_s', 0) for r in prior if r.get('stage') == 'preparation')
        if min(86400 - total, 14400 - frontier, 3540 - stage) < 240 + precheck_s:
            raise RuntimeError('240-second preparation slice does not fit remaining allocation')
        if shutil.disk_usage(os.environ['TMPDIR']).free < 16 * 1024**3:
            raise RuntimeError('data drive free reserve unavailable')
        before = coupled_admit.observe(512 * 1024, 0)
        out = coord / ID
        out.mkdir()
        work = Path(os.environ['TMPDIR']) / ID
        work.mkdir()
        relative = next(s[3:] for s in Path('/proc/self/cgroup').read_text().splitlines() if s.startswith('0::'))
        group = Path('/sys/fs/cgroup') / relative.lstrip('/')
        record = dict(id=ID, lab='coordinator', stage='preparation', state='running',
            command=sys.argv, timeout_s=240, stage_limit_s=3540, stage_prior_execution_s=stage,
            prior_execution_s=total, frontier_limit_s=14400, frontier_prior_execution_s=frontier,
            preflight_receipts=prechecks, preflight_charged_s=precheck_s,
            cgroup=relative, scratch=str(work), started_unix_ns=time.time_ns(), before=before,
            commands=[], source_sha256={str(p.relative_to(ROOT)): common.sha(p) for p in
                (Path(__file__).resolve(), PROTOCOL, Path(archive.__file__).resolve(),
                 ROOT / 'tools/bench/labs/catalog/range_closeout.py',
                 ROOT / 'tools/bench/labs/catalog/coupled_admit.py')},
            limits={name: (group / name).read_text().strip() for name in
                ('memory.max', 'memory.high', 'memory.swap.max', 'cpu.max')})
        shutil.copyfile(__file__, out / 'driver.py.txt')
        shutil.copyfile(PROTOCOL, out / 'protocol.txt')
        common.dump(out / 'receipt.json', record)

        def run(argv, name, seconds):
            tick = time.monotonic()
            log = out / (name + '.txt')
            with log.open('wb') as stream:
                process = subprocess.Popen(argv, cwd=ROOT, stdout=stream, stderr=subprocess.STDOUT,
                    env=dict(os.environ, FABRIC_SCRATCH_ROOT=str(work), RUST_TEST_THREADS='1'),
                    start_new_session=True)
                try:
                    while process.poll() is None:
                        if time.monotonic() - tick > seconds or time.monotonic() - started > 230:
                            raise TimeoutError(name + ' deadline')
                        if log.stat().st_size > 64 * 1024:
                            raise RuntimeError(name + ' output exceeded bound')
                        time.sleep(.05)
                    code = process.returncode
                except BaseException:
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    process.wait()
                    raise
                finally:
                    record['commands'].append(dict(argv=argv, exit=process.returncode,
                        elapsed_s=time.monotonic() - tick, log=log.name))
            if code or log.stat().st_size > 64 * 1024:
                raise RuntimeError(f'{name} failed: exit {code}')

        code = 1
        try:
            same_source(b'same', b'same')
            try:
                same_source(b'same', b'changed')
            except RuntimeError:
                record['source_change_control_rejected'] = True
            else:
                raise RuntimeError('changed-source control accepted')
            record['archive_controls_rejected'] = archive.archive_controls(work)
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
                raise RuntimeError('empty source comparison')
            record.update(unchanged_product_source_sha256=checked,
                reused_source_archive=str(source_path.relative_to(ROOT)),
                reused_source_archive_sha256=common.sha(source_path))
            run(['cargo', 'test', '--offline', '--locked', '-p', 'fabric-server', '--lib',
                 'sealer::tests'], 'sealer-tests', 150)
            record['after_tests'] = {name: (group / name).read_text().strip() for name in
                ('memory.peak', 'memory.events', 'cpu.stat', 'io.stat')}
            downloaded = work / 'turso.tar.gz'
            url = f'https://codeload.github.com/tursodatabase/turso/tar.gz/{PIN}'
            run(['curl', '--fail', '--location', '--max-time', '60', '--max-filesize',
                 str(64 * 1024**2), '--output', str(downloaded), url], 'turso-download', 65)
            upstream = json.loads((ROOT / 'docs/research/turso-source-manifest.json').read_text())
            expected = {r['path']: r['git_blob_sha'] for r in upstream['files']
                        if r['retrieval'] == 'complete file'}
            selected = {'core/storage/page_cache.rs', 'core/storage/buffer_pool.rs',
                        'core/translate/optimizer/cost_params.rs',
                        'docs/agent-guides/async-io-model.md', 'testing/simulator/README.md'}
            manifest, snapshots, checked_blobs = {}, {}, {}
            decoded = 0
            with tarfile.open(downloaded, 'r:gz') as source:
                for member in source:
                    if not member.isfile():
                        continue
                    decoded += member.size
                    if decoded > 512 * 1024**2 or not member.name.startswith('turso-' + PIN + '/'):
                        raise RuntimeError('upstream archive outside registered bounds')
                    name = member.name.split('/', 1)[1]
                    if name in manifest or member.size > 64 * 1024**2:
                        raise RuntimeError('duplicate or oversized upstream member')
                    payload = source.extractfile(member).read()
                    manifest[name] = dict(bytes=len(payload), sha256=hashlib.sha256(payload).hexdigest())
                    if name in expected:
                        blob = hashlib.sha1(b'blob ' + str(len(payload)).encode() + b'\0' + payload).hexdigest()
                        if blob != expected[name]:
                            raise RuntimeError('upstream blob differs from source review: ' + name)
                        checked_blobs[name] = blob
                    if name in selected:
                        snapshots[name] = payload.decode('utf-8')
            if set(checked_blobs) != set(expected) or set(snapshots) != selected:
                raise RuntimeError('upstream source coverage missing')
            compressed(out / 'turso-members.json.gz', json.dumps(manifest, sort_keys=True).encode())
            compressed(out / 'turso-selected-source.json.gz', json.dumps(snapshots, sort_keys=True).encode())
            record['turso'] = dict(url=url, revision=PIN, archive_sha256=common.sha(downloaded),
                archive_bytes=downloaded.stat().st_size, decoded_bytes=decoded,
                regular_files=len(manifest), matched_prior_git_blobs=checked_blobs,
                selected_source_readback=True, built=False, extracted_checkout=False)
            if checked != {name: common.sha(ROOT / name) for name in checked}:
                raise RuntimeError('product source changed during execution')
            if sum(max(p.stat().st_size, p.stat().st_blocks * 512) for p in out.iterdir()) + 65536 > 512 * 1024:
                raise RuntimeError('retained evidence exceeds reservation')
            shutil.rmtree(work)
            record['state'] = 'passed'
            code = 0
        except BaseException as error:
            record.update(state='failed', error=repr(error))
            if work.exists():
                record['retained_failure_scratch'] = str(work)
            raise
        finally:
            record['scratch_removed'] = not work.exists()
            record['cgroup_final'] = {name: (group / name).read_text().strip() for name in
                ('memory.peak', 'memory.events', 'memory.swap.current', 'cpu.stat', 'io.stat')}
            record['exit'] = code
            # Admission scans themselves consume runtime and belong to this receipt.
            try:
                record['after'] = coupled_admit.observe(0, 0)
            finally:
                record['execution_elapsed_s'] = time.monotonic() - started
                record['elapsed_s'] = record['execution_elapsed_s'] + precheck_s
                common.dump(out / 'receipt.json', record)


if __name__ == '__main__':
    main()
