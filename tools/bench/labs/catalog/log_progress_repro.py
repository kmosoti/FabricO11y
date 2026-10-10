"""Reproduce the registered skip-only stall and preserve its failing fixture."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import tarfile
import time

import coupled_overlap as common

ROOT = common.ROOT
ID = 'catalog-log-progress-repro-01'
OUT = ROOT / 'docs/experiments/benchmarks/data' / ID
RUST = ('src/spindle/runtime.rs', 'src/spindle/skip_progress_tests.rs')
SOURCES = (*RUST, 'src/spindle/spool.rs', 'crates/fabric-adapter-linux/src/log_source.rs',
           'tools/bench/labs/catalog/log_progress_repro.py', 'Cargo.lock')


def archive_fixture(work):
    paths = {}
    for p in sorted(work.rglob('*')):
        if p.is_symlink() or not (p.is_dir() or p.is_file()):
            raise RuntimeError('unexpected fixture entry')
        if p.is_file():
            paths[str(p.relative_to(work))] = {'bytes': p.stat().st_size, 'sha256': common.sha(p)}
    if sum(p['bytes'] for p in paths.values()) > 4 * 1024**2:
        raise RuntimeError('fixture exceeds registered small preservation bound')
    archive = OUT / 'counterexample.tar.gz'
    with tarfile.open(archive, 'x:gz') as stream:
        for relative in paths:
            stream.add(work / relative, arcname=relative, recursive=False)
    with tarfile.open(archive, 'r:gz') as stream:
        members = stream.getmembers()
        if len(members) != len(paths) or {m.name for m in members} != set(paths):
            raise RuntimeError('archive member coverage mismatch')
        for member in members:
            if not member.isfile() or member.size != paths[member.name]['bytes']:
                raise RuntimeError('archive entry type/size mismatch')
            digest = hashlib.sha256()
            with stream.extractfile(member) as decoded, (work / member.name).open('rb') as original:
                while True:
                    a, b = decoded.read(65536), original.read(65536)
                    if a != b:
                        raise RuntimeError('archive exact readback mismatch')
                    digest.update(a)
                    if not a:
                        break
            if digest.hexdigest() != paths[member.name]['sha256']:
                raise RuntimeError('archive content digest mismatch')
    common.dump(OUT / 'counterexample-manifest.json', {
        'archive_sha256': common.sha(archive), 'members': paths, 'exact_readback': True})


def expected_counterexample(trace):
    passes = trace.get('passes', [])
    return (trace.get('input_bytes') == 3145746 and len(passes) == 2
        and passes[0]['returned_cycle']['sequence'] == 1
        and passes[0]['returned_cycle']['logs'] == 0
        and passes[0]['returned_cycle']['gaps'] == 1
        and passes[0]['cursor'] == {'offset': 1048576, 'skipping_oversize': True}
        and passes[0]['acked_after'] == 1 and passes[1]['returned_cycle'] is None
        and passes[1]['cursor'] == {'offset': 1048576, 'skipping_oversize': True}
        and passes[1]['acked'] == 1 and passes[1]['next_sequence'] == 2)


def main():
    common.require_limits()
    OUT.mkdir(exist_ok=False)
    work = Path(os.environ['FABRIC_SCRATCH_ROOT']) / 'skip-reproduction'
    work.mkdir()
    env = dict(os.environ, FABRIC_SCRATCH_ROOT=str(work))
    deadline = time.monotonic() + 150
    receipt = {'state': 'running', 'commands': [], 'scratch': str(work)}
    def execute(label, argv):
        code = common.profile.run_child(argv, env, OUT / (label + '.stdout'),
            OUT / (label + '.stderr'), deadline, work, OUT)
        receipt['commands'].append({'argv': argv, 'exit': code})
        common.dump(OUT / 'receipt.json', receipt)
        return code
    try:
        if execute('format', ['rustfmt', '--edition', '2024', *RUST]):
            raise RuntimeError('format failed')
        frozen = {p: common.sha(ROOT / p) for p in SOURCES}
        receipt['source_sha256'] = frozen
        for p in SOURCES:
            target = OUT / 'source' / p
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / p, target)
        shutil.copyfile(ROOT / 'docs/experiments/benchmarks/catalog-log-progress-protocol.md',
                        OUT / 'protocol.txt')
        code = execute('regression', ['cargo', 'test', '--offline', '--locked', '-p',
            'fabric_o11y', '--lib',
            'skip_only_passes_commit_forward_progress_without_duplicate_gap_or_suffix',
            '--', '--nocapture', '--test-threads=1'])
        traces = list(work.glob('skip-progress-*/trace.json'))
        if len(traces) != 1:
            raise RuntimeError('expected exactly one failed fixture trace')
        trace = json.loads(traces[0].read_text())
        if code != 101 or not expected_counterexample(trace):
            raise RuntimeError('registered counterexample not reproduced as expected')
        # A different, progressing cursor must not be called this counterexample.
        changed = json.loads(json.dumps(trace))
        changed['passes'][1]['cursor']['offset'] = 2097152
        if expected_counterexample(changed):
            raise RuntimeError('counterexample classifier accepted progress control')
        common.dump(OUT / 'trace.json', trace)
        archive_fixture(work)
        if frozen != {p: common.sha(ROOT / p) for p in SOURCES}:
            raise RuntimeError('reproduction source drift')
        allocated = sum(max(p.stat().st_size, p.stat().st_blocks * 512)
                        for p in OUT.rglob('*') if p.is_file())
        if allocated > 512 * 1024:
            raise RuntimeError('512KiB evidence cap exceeded')
        receipt.update(state='counterexample_reproduced', regression_exit=code,
            original_rust_check='failed', progressing_control_rejected=True,
            allocated_evidence_bytes=allocated, archive_exact_readback=True)
        shutil.rmtree(work)
    except BaseException as error:
        receipt.update(state='failed', error=repr(error))
        raise
    finally:
        receipt['scratch_removed'] = not work.exists()
        common.dump(OUT / 'receipt.json', receipt)


if __name__ == '__main__':
    main()
