"""Registered startup counterexample and native lifecycle execution."""
import argparse
import ast
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import tarfile
import time

import algorithm_round as parsing

common = parsing.common
ROOT = common.ROOT
SOURCES = ('crates/fabric-server/src/lib.rs', 'crates/fabric-server/src/store.rs',
           'crates/fabric-server/src/sealer.rs', 'crates/fabric-server/src/rows.rs',
           'crates/fabric-server/tests/startup.rs', 'src/spindle/runtime.rs',
           'src/spindle/spool.rs', 'Cargo.toml', 'Cargo.lock',
           'tools/bench/labs/catalog/native_lifecycle_run.py')


def verify_archive(work, archive, manifest):
    with tarfile.open(archive, 'r:gz') as stream:
        members = stream.getmembers()
        if len(members) != len(manifest) or {m.name for m in members} != set(manifest):
            raise RuntimeError('archive coverage mismatch')
        for member in members:
            if not member.isfile() or member.size != manifest[member.name]['bytes']:
                raise RuntimeError('archive member type/size mismatch')
            digest = hashlib.sha256()
            with stream.extractfile(member) as decoded, (work / member.name).open('rb') as original:
                while True:
                    left, right = decoded.read(65536), original.read(65536)
                    if left != right:
                        raise RuntimeError('archive byte readback mismatch')
                    digest.update(left)
                    if not left:
                        break
            if digest.hexdigest() != manifest[member.name]['sha256']:
                raise RuntimeError('archive digest mismatch')


def archive_controls(work):
    controls = work / 'archive-controls'
    controls.mkdir()
    (controls / 'sample').write_bytes(b'abc')
    manifest = {'sample': {'bytes': 3, 'sha256': common.sha(controls / 'sample')}}
    rejected = []
    for variant in ('valid', 'altered', 'missing', 'duplicate'):
        archive = controls / (variant + '.tar.gz')
        with tarfile.open(archive, 'x:gz') as stream:
            for _ in range(0 if variant == 'missing' else 2 if variant == 'duplicate' else 1):
                member = tarfile.TarInfo('sample')
                member.size = 3
                stream.addfile(member, io.BytesIO(b'bad' if variant == 'altered' else b'abc'))
        try:
            verify_archive(controls, archive, manifest)
        except RuntimeError:
            if variant == 'valid':
                raise
            rejected.append(variant)
        else:
            if variant != 'valid':
                raise RuntimeError('archive negative control accepted: ' + variant)
    shutil.rmtree(controls)
    return rejected


def preserve(work, out, maximum):
    manifest = {}
    for path in sorted(work.rglob('*')):
        if path.is_symlink() or not (path.is_file() or path.is_dir()):
            raise RuntimeError('unexpected fixture entry')
        if path.is_file():
            manifest[str(path.relative_to(work))] = {
                'bytes': path.stat().st_size, 'sha256': common.sha(path)}
    if sum(p['bytes'] for p in manifest.values()) > maximum:
        raise RuntimeError('full fixture exceeds preservation bound; retain original')
    archive = out / 'fixture.tar.gz'
    with tarfile.open(archive, 'x:gz') as stream:
        for name in manifest:
            stream.add(work / name, arcname=name, recursive=False)
    verify_archive(work, archive, manifest)
    result = {'members': manifest, 'archive_sha256': common.sha(archive),
              'exact_readback': True, 'decoded_file_bytes': sum(p['bytes'] for p in manifest.values())}
    common.dump(out / 'fixture-manifest.json', result)
    return result


def startup_counterexample(trace):
    attempts = trace.get('reopen_attempts', [])
    return (trace.get('case') in ('missing_certificate', 'malformed_key')
            and trace.get('pre_start_store_open') is True
            and bool(trace.get('serve_error', {}).get('message'))
            and len(attempts) >= 2
            and all(row.get('opened') is False and row.get('kind') == 'WouldBlock'
                    for row in attempts))


def main():
    common.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mode', choices=('repro', 'lifecycle'), required=True)
    args = parser.parse_args()
    reproduction = args.mode == 'repro'
    name = 'catalog-startup-repro-01' if reproduction else 'catalog-native-lifecycle-01'
    out = ROOT / 'docs/experiments/benchmarks/data' / name
    out.mkdir(exist_ok=False)
    work = Path(os.environ['FABRIC_SCRATCH_ROOT']) / name
    work.mkdir()
    env = dict(os.environ, FABRIC_SCRATCH_ROOT=str(work))
    for key in ('FABRIC_ACK_ADVANCE_EXPERIMENT', 'FABRIC_SPILL_WORKSPACE_EXPERIMENT',
                'FABRIC_RUN_MIB_EXPERIMENT', 'FABRIC_BORROWED_LOG_EXPERIMENT'):
        env.pop(key, None)
    deadline = time.monotonic() + (150 if reproduction else 320)
    cap = 512 * 1024 if reproduction else 18 * 1024**2
    sources = (*SOURCES, *(() if reproduction else (
        'crates/fabric-server/tests/native_lifecycle.rs',
        'crates/fabric-server/tests/delivery.rs',
        'tools/qualification/query_oracle.py')))
    receipt = {'state': 'running', 'commands': [], 'scratch': str(work)}
    common.dump(out / 'receipt.json', receipt)
    def execute(label, argv):
        code = common.profile.run_child(argv, env, out / (label + '.stdout'),
            out / (label + '.stderr'), deadline, work, out)
        receipt['commands'].append({'argv': argv, 'exit': code})
        common.dump(out / 'receipt.json', receipt)
        return code
    try:
        ast.parse(Path(__file__).read_text())
        formatting = ['crates/fabric-server/tests/startup.rs']
        if not reproduction:
            formatting.append('crates/fabric-server/tests/native_lifecycle.rs')
        if execute('format', ['rustfmt', '--edition', '2024', *formatting]):
            raise RuntimeError('format failed')
        frozen = {p: common.sha(ROOT / p) for p in sources}
        receipt['source_sha256'] = frozen
        receipt['rejected_archive_controls'] = archive_controls(work)
        for path in sources:
            destination = out / 'source' / path
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / path, destination)
        shutil.copyfile(ROOT / 'docs/experiments/benchmarks/catalog-startup-recovery-protocol.md',
                        out / 'protocol.txt')
        code = execute('startup', ['cargo', 'test', '--offline', '--locked', '-p',
            'fabric-server', '--test', 'startup', '--', '--nocapture', '--test-threads=1'])
        if reproduction:
            traces = [json.loads(p.read_text()) for p in sorted(work.glob('startup-*/trace.json'))]
            if (code != 101 or len(traces) != 2
                    or {t.get('case') for t in traces} != {'missing_certificate', 'malformed_key'}
                    or not all(startup_counterexample(t) for t in traces)):
                raise RuntimeError('expected startup counterexample not reproduced')
            changed = json.loads(json.dumps(traces[0]))
            changed['reopen_attempts'].append({'opened': True})
            if startup_counterexample(changed):
                raise RuntimeError('counterexample classifier accepted released writer')
            common.dump(out / 'traces.json', traces)
            receipt.update(state='counterexample_reproduced', original_rust_exit=code,
                           original_rust_checks='failed', reopened_control_rejected=True)
            preserve(work, out, 1024**2)
        else:
            if code:
                raise RuntimeError('corrected startup controls failed')
            # The native fixture is separately registered before this mode runs.
            shutil.copyfile(ROOT / 'docs/experiments/benchmarks/catalog-native-lifecycle-protocol.md',
                            out / 'lifecycle-protocol.txt')
            env['FABRIC_NATIVE_EVIDENCE'] = str(work / 'completed')
            code = execute('native', ['cargo', 'test', '--offline', '--locked', '-p',
                'fabric-server', '--test', 'native_lifecycle', '--', '--nocapture', '--test-threads=1'])
            if code:
                raise RuntimeError('native lifecycle control failed')
            finals = list(work.glob('native-lifecycle-*/*-final.json'))
            if len(finals) != 1:
                raise RuntimeError('exact native final summary missing')
            summary = json.loads(finals[0].read_text())
            if not (summary.get('exact') is True and summary.get('query_chains') == 30
                    and summary.get('negative_query_controls') == 12
                    and summary.get('source_log_rows') == 41
                    and summary.get('producer_batches') == 7 and summary.get('segments', 0) > 0):
                raise RuntimeError('native final coverage mismatch')
            common.dump(out / 'summary.json', summary)
            code = execute('delivery', ['cargo', 'test', '--offline', '--locked', '-p',
                'fabric-server', '--test', 'delivery', '--', '--test-threads=1'])
            if code:
                raise RuntimeError('existing TLS delivery integration failed')
            receipt['fixture'] = preserve(work, out, 16 * 1024**2)
            receipt['state'] = 'complete'
        if frozen != {p: common.sha(ROOT / p) for p in sources}:
            raise RuntimeError('source changed during execution')
        allocated = sum(max(p.stat().st_size, p.stat().st_blocks * 512)
                        for p in out.rglob('*') if p.is_file())
        if allocated > cap:
            raise RuntimeError('registered driver evidence cap exceeded')
        receipt['allocated_evidence_bytes'] = allocated
        shutil.rmtree(work)
    except BaseException as error:
        receipt.update(state='failed', error=repr(error))
        if not reproduction and work.exists() and not (out / 'fixture.tar.gz').exists():
            try:
                receipt['failure_fixture'] = preserve(work, out, 16 * 1024**2)
                shutil.rmtree(work)
            except BaseException as preservation_error:
                # Keep original failure and the raw tree if preservation fails.
                receipt['preservation_error'] = repr(preservation_error)
        raise
    finally:
        receipt['scratch_removed'] = not work.exists()
        common.dump(out / 'receipt.json', receipt)


if __name__ == '__main__':
    main()
