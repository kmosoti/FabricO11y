"""Isolated regressions for actual sweep artifact and receipt code.

Root executes this helper through the existing serialized resource launcher.
Imports actual scripts; no networking, downloaded-code execution or Fabric build.
"""
import argparse
from contextlib import ExitStack
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import sys
import tarfile
import time
from unittest import mock

import prefix_load as shared
import query_sweep
import run_sweep_job as runner
import source_repair

MIB = 1024 ** 2


def require(value, message):
    if not value:
        raise RuntimeError(message)


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def retained_copy_controls(work):
    work.mkdir()
    source, target = work / 'source', work / 'target'
    source.write_bytes(b'exact-copy-fixture')
    query_sweep.retained_copy(source, target)
    require(source.read_bytes() == target.read_bytes(), 'valid actual retained_copy failed')
    records = [{'variant': 'valid', 'accepted': True}]
    for name, payload in [('altered-same-size', b'X' * source.stat().st_size), ('short-copy', b'exact')]:
        target.unlink()
        def faulty_copy(old, new, **kwargs):
            Path(new).write_bytes(payload)
            return str(new)
        with mock.patch.object(query_sweep.shutil, 'copy2', faulty_copy):
            try:
                query_sweep.retained_copy(source, target)
            except RuntimeError as error:
                records.append({'variant': name, 'rejected': True, 'error': repr(error),
                    'source_sha256': shared.sha(source), 'partial_target_sha256': shared.sha(target)})
            else:
                raise RuntimeError('actual retained_copy accepted ' + name)
    return records


def source_copy_control(work):
    repo_root, scratch = work / 'root', work / 'scratch'
    scratch.mkdir(parents=True)
    repo_root.mkdir()
    catalog = repo_root / 'catalog.json'
    catalog.write_text(json.dumps({'repositories': [{'id': 'vector', 'repo': 'fixture/vector'}]}))
    dest = repo_root / 'docs/experiments/benchmarks/data/cross-system-sweep-01/source'
    uncompressed = io.BytesIO()
    with tarfile.open(fileobj=uncompressed, mode='w') as archive:
        row = tarfile.TarInfo('repo/file.rs')
        payload = b'// fixed source-copy custody fixture\n'
        row.size = len(payload)
        archive.addfile(row, io.BytesIO(payload))
    complete = gzip.compress(uncompressed.getvalue(), mtime=0)
    original_copy = source_repair.shutil.copyfile
    fault_calls = []
    def fake_download(repo, pin, path, evidence, deadline):
        Path(path).write_bytes(complete)
    def fake_census(archive, spec, evidence, deadline):
        require(Path(archive).read_bytes() == complete, 'fake census input differs')
        return {'regular_members': 1, 'fixture_census': True}
    def partial_success_copy(old, new, **kwargs):
        if Path(new).name == 'upstream.tar.gz':
            raw = Path(old).read_bytes()
            Path(new).write_bytes(raw[:len(raw) // 2])
            fault_calls.append(str(new))
            return str(new)  # I/O returned success, but copied only a prefix.
        return original_copy(old, new, **kwargs)
    fetch = source_repair.fetch
    with ExitStack() as patches:
        for name, value in [('ROOT', repo_root), ('CATALOG', catalog),
                            ('download', fake_download), ('census', fake_census)]:
            patches.enter_context(mock.patch.object(fetch, name, value))
        # Actual cgroup limits remain required. Admission's free-space census is
        # real; only source response/census and the preservation copy are faults.
        for name in ('COMPRESSED', 'DECODED', 'INVENTORY'):
            patches.enter_context(mock.patch.object(fetch, name, getattr(fetch, name)))
        patches.enter_context(mock.patch.object(source_repair.shutil, 'copyfile', partial_success_copy))
        patches.enter_context(mock.patch.dict(os.environ, FABRIC_SCRATCH_ROOT=str(scratch)))
        patches.enter_context(mock.patch.object(sys, 'argv', ['source_repair.py', '--destination', str(dest), '--repos', 'vector']))
        try:
            source_repair.main()
        except RuntimeError as error:
            require('complete archive preservation mismatch' in str(error), 'unexpected repair fault')
        else:
            raise RuntimeError('source_repair accepted partial successful preservation copy')
    receipt_path = dest / 'vector/receipt.json'
    receipt = json.loads(receipt_path.read_text())
    original = Path(receipt['scratch']) / 'upstream.tar.gz'
    partial = dest / 'vector/upstream.tar.gz'
    require(receipt['state'] == 'failed' and receipt['exit_code'] == 1, 'repair did not record failed copy')
    require(original.read_bytes() == complete and receipt['retained_original_in_scratch'], 'complete original lost')
    require(not receipt['scratch_removed'] and len(fault_calls) == 1, 'repair retried duplicated copy')
    require(not (dest / 'vector/failed-download.tar.gz').exists(), 'repair allocated duplicate failed-download copy')
    require(receipt['preservation_partial_sha256'] == shared.sha(partial), 'partial-copy receipt hash mismatch')
    return {'origin': 'actual source_repair.main partial-success preservation-copy branch',
        'network_mocked': True, 'census_mocked': True, 'preservation_copy_fault': 'short prefix with success return',
        'original_sha256': digest(complete), 'partial_sha256': shared.sha(partial),
        'original_retained': True, 'duplicate_failed_download_absent': True, 'receipt': receipt}


def runner_control(work, raw_leftover):
    root = work / 'root'
    (root / 'target').mkdir(parents=True)
    scratch = work / 'scratch'
    scratch.mkdir()
    base = root / 'docs/experiments/benchmarks/data/cross-system-sweep-01'
    old = root / 'docs/experiments/benchmarks/data/lab-completion-run-01/coordinator'
    protocol = root / 'fixture-protocol.md'
    protocol.write_text('isolated receipt regression fixture\n')
    child_started, stop_calls = [False], []
    original_spawn, original_footprint = runner.subprocess.Popen, runner.footprint
    def spawn(*args, **kwargs):
        child_started[0] = True
        return original_spawn(*args, **kwargs)
    def census(path):
        if raw_leftover and child_started[0] and Path(path) == base / 'query':
            raise RuntimeError('linked continuation evidence/scratch')
        return original_footprint(path)
    child_code = ('import os;from pathlib import Path;'
                  'Path(os.environ["FABRIC_SCRATCH_ROOT"]).joinpath("owned-raw").write_bytes(b"retain-me")') if raw_leftover else 'pass'
    identifier = 'leftover-final-census' if raw_leftover else 'positive-empty-child'
    argv = ['run_sweep_job.py', '--id', identifier, '--lab', 'query', '--seconds', '20',
            '--reserve-mib', '0', '--', sys.executable, '-B', '-c', child_code]
    with ExitStack() as patches:
        for name, value in [('ROOT', root), ('BASE', base), ('OLD', old), ('PROTOCOL', protocol),
                            ('footprint', census), ('stop_descendants', lambda group: stop_calls.append(str(group)))]:
            patches.enter_context(mock.patch.object(runner, name, value))
        patches.enter_context(mock.patch.object(runner.coupled_admit, 'observe', lambda *args: {'isolated_admission_fixture': True}))
        patches.enter_context(mock.patch.object(runner.subprocess, 'Popen', spawn))
        patches.enter_context(mock.patch.dict(os.environ, TMPDIR=str(scratch), FABRIC_SCRATCH_ROOT=str(scratch)))
        patches.enter_context(mock.patch.object(sys, 'argv', argv))
        code = runner.main()
    receipt = json.loads((base / 'coordinator' / identifier / 'receipt.json').read_text())
    original = scratch / identifier
    require(receipt['child_exit'] == 0, 'tiny child did not finish successfully')
    require(stop_calls, 'actual coordinator did not reach descendant finalization')
    if raw_leftover:
        require(code == 1 and receipt['state'] == 'failed' and receipt['exit'] == 1,
                'final census failure did not write failed receipt/exit1')
        require('linked continuation evidence/scratch' in receipt['evidence_census_error'], 'final census error omitted')
        require(not receipt['scratch_removed'] and (original / 'owned-raw').read_bytes() == b'retain-me',
                'pre-existing failed child scratch was removed')
        require('command left owned scratch' in receipt['error'], 'primary failure boundary differs')
    else:
        require(code == 0 and receipt['state'] == 'passed' and receipt['scratch_removed'] and not original.exists(),
                'positive empty-child cleanup failed')
    return {'origin': 'actual run_sweep_job.main with isolated ledger/readiness lock',
        'real_tiny_child': True, 'descendant_termination_mocked': True,
        'admission_observation_mocked': True, 'raw_leftover': raw_leftover, 'receipt': receipt,
        'boundary': 'leftover scratch fails before final census; this tests receipt finalization of that failure'
            if raw_leftover else 'empty successful child is cleaned before final census'}


def main():
    require(os.environ.get('FABRIC_CROSS_SYSTEM_COORDINATED') == '1', 'root coordinator ownership required')
    shared.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--proposal', type=Path, required=True)
    args = parser.parse_args()
    require(not args.out.exists() and args.proposal.is_file(), 'fresh output/registered supplement required')
    args.out.mkdir(parents=True)
    work = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True) / 'artifact-regression-controls'
    work.mkdir()
    started = time.monotonic()
    record = {'state': 'running', 'scratch': str(work), 'commands_scope': 'actual scripts, isolated roots, no network',
        'helper_sha256': shared.sha(Path(__file__)),
        'proposal_sha256': shared.sha(args.proposal), 'sources': {str(Path(m.__file__).resolve()): shared.sha(Path(m.__file__))
            for m in (query_sweep, source_repair, source_repair.fetch, runner)}}
    for i, source in enumerate([Path(__file__).resolve(), args.proposal,
                               *[Path(path) for path in record['sources']]]):
        raw = source.read_bytes()
        dest = args.out / f'source-{i:02}.gz'
        dest.write_bytes(gzip.compress(raw, mtime=0))
        require(gzip.decompress(dest.read_bytes()) == raw, 'source readback mismatch')
    shared.dump(args.out / 'receipt.json', record)
    try:
        record['retained_copy_controls'] = retained_copy_controls(work / 'copy')
        record['source_copy_control'] = source_copy_control(work / 'source')
        record['runner_positive'] = runner_control(work / 'runner-positive', False)
        record['runner_failure'] = runner_control(work / 'runner-failure', True)
        require(time.monotonic() - started < 50, 'finite regression helper deadline')
        require(shared.footprint(work) < 3 * MIB, 'regression raw fixture exceeds3MiB')
        record['retained_fixture'] = shared.archive.preserve(work, args.out, 3 * MIB)
        require(shared.footprint(args.out) < 4 * MIB, 'regression evidence exceeds4MiB')
        # Failure-like fixtures become removable only after the actual expected
        # failure assertions and complete archival readback both succeed.
        shutil.rmtree(work)
        record.update(state='complete', scratch_removed=not work.exists())
    except BaseException as error:
        record.update(state='failed', error=repr(error), scratch_removed=False)
        raise
    finally:
        record['elapsed_s'] = time.monotonic() - started
        shared.dump(args.out / 'receipt.json', record)
    print(json.dumps({'state': record['state'], 'receipt': str(args.out / 'receipt.json')}))


if __name__ == '__main__':
    main()
