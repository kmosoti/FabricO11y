"""Fetch bounded requested implementation files at already-recorded revisions."""
import gzip
import json
import os
from pathlib import Path
from pathlib import PurePosixPath
import select
import shutil
import subprocess
import sys
import tarfile
import tempfile
import re
import argparse
import time

from source_fetch import require_limits, digest, EVIDENCE, size, archive_readback

ROOT = Path(__file__).resolve().parents[4]
BASE = ROOT / 'docs/experiments/benchmarks/data/cross-system-run-01/source'
REQUESTS = {
    'vector-current': ['src/sinks/util/adaptive_concurrency/controller.rs',
                       'src/sinks/util/adaptive_concurrency/future.rs', 'src/sinks/util/buffer/service.rs', 'LICENSE'],
    'foundationdb-current': ['flow/DeterministicRandom.cpp', 'flow/include/flow/DeterministicRandom.h',
                             'flow/Net2.actor.cpp', 'fdbrpc/SimulatedNetwork.actor.cpp',
                             'flow/include/flow/IRandom.h', 'LICENSE'],
    'shuttle': ['shuttle-engine/src/scheduler/mod.rs', 'shuttle-schedulers/src/dfs.rs',
                'shuttle-schedulers/src/random.rs', 'shuttle-schedulers/src/replay.rs'],
    'differential_dataflow': ['differential-dataflow/src/trace/implementations/ord_neu.rs'],
    'slatedb': ['slatedb/src/manifest/store.rs', 'slatedb/src/db.rs'],
    'tantivy': ['src/tokenizer/ngram_tokenizer.rs', 'src/tokenizer/simple_tokenizer.rs',
                'src/index/inverted_index_reader.rs'],
    'duckdb': ['src/storage/table/column_data.cpp', 'src/storage/table/row_group_reorderer.cpp'],
    'quickwit': ['quickwit/quickwit-indexing/src/actors/publisher.rs'],
    'tempo': ['tempodb/encoding/vparquet5/index.go', 'tempodb/encoding/vparquet5/create.go'],
    'roaring': ['roaring/src/bitmap/store/mod.rs'],
    'clickhouse': ['src/Storages/MergeTree/MergeTreeDataSelectExecutor.cpp',
                  'src/Storages/MergeTree/MergeTreeIndexBloomFilter.cpp',
                  'src/Storages/MergeTree/MergeTreeIndexBloomFilter.h',
                  'src/Storages/MergeTree/MergeTreeIndexSet.cpp',
                  'src/Storages/MergeTree/MergeTreeIndexMinMax.cpp',
                  'src/Storages/MergeTree/MergeTreeIndexGranuleBloomFilter.cpp',
                  'LICENSE'],
}

FILE_LIMIT = 8 * 1024**2
GROUP_LIMIT = 24 * 1024**2
ERROR_LIMIT = 64 * 1024


def durable_json(path, value):
    temporary = path.with_suffix('.pending')
    with temporary.open('w') as stream:
        stream.write(json.dumps(value, indent=2) + '\n')
        stream.flush()
        os.fsync(stream.fileno())
    temporary.replace(path)
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def validate_pin(revision):
    if not isinstance(revision, str) or not re.fullmatch('[0-9a-f]{40}', revision):
        raise RuntimeError('supplement revision is not an immutable40hex SHA')


def download_file(url, target, error):
    """Retain bounded response/error prefixes even on timeout or curl failure."""
    command = ['curl', '--fail', '--location', '--silent', '--show-error', '--connect-timeout', '10',
               '--max-time', '30', '--max-filesize', str(FILE_LIMIT), url]
    child, problem, total, error_bytes = None, None, 0, 0
    try:
        with target.open('wb') as output, error.open('wb') as stderr:
            try:
                child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                active = {child.stdout: output, child.stderr: stderr}
                deadline = time.monotonic() + 35
                while active:
                    if time.monotonic() >= deadline:
                        raise RuntimeError('supplement35-second file deadline')
                    ready, _, _ = select.select(list(active), [], [], .25)
                    for source in ready:
                        block = os.read(source.fileno(), 64 * 1024)
                        if not block:
                            del active[source]
                            continue
                        count = total if source is child.stdout else error_bytes
                        limit = FILE_LIMIT if source is child.stdout else ERROR_LIMIT
                        retained = block[:max(0, limit - count)]
                        active[source].write(retained)
                        if source is child.stdout:
                            total += len(retained)
                        else:
                            error_bytes += len(retained)
                        if len(retained) != len(block):
                            raise RuntimeError('supplement bounded prefix ceiling')
                child.wait(timeout=max(.001, deadline - time.monotonic()))
            except Exception as failure:
                problem = repr(failure)
            finally:
                if child is not None and child.poll() is None:
                    child.kill()
                    child.wait()
                output.flush()
                stderr.flush()
                os.fsync(output.fileno())
                os.fsync(stderr.fileno())
    finally:
        code = child.returncode if child is not None else 1
    return {'url': url, 'argv': command, 'exit': (code or 1) if problem else code,
            'bytes': total, 'sha256': digest(target), 'error': error.read_text(errors='replace'),
            'scope_error': problem, 'prefix_truncated': bool(problem),
            'staged_source': str(target), 'staged_error': str(error)}


def main():
    require_limits()
    parser = argparse.ArgumentParser()
    parser.add_argument('--recovery-paths', action='store_true')
    args = parser.parse_args()
    requests = REQUESTS
    if args.recovery_paths:
        requests = {
            'foundationdb-current': ['fdbrpc/sim2.cpp', 'fdbrpc/include/fdbrpc/simulator.h',
                                     'fdbrpc/AsyncFileNonDurable.cpp',
                                     'fdbrpc/include/fdbrpc/AsyncFileNonDurable.h'],
            'vector-current': ['lib/vector-buffers/src/variants/disk_v2/' + name
                               for name in ('mod.rs', 'checkpoint_recovery.rs', 'writer.rs', 'reader.rs', 'ledger.rs')],
        }
    out = BASE / ('supplements-02' if args.recovery_paths else 'supplements-01')
    if size(BASE) + 32 * 1024**2 > EVIDENCE:
        raise RuntimeError('source evidence allowance unavailable')
    out.mkdir()
    work = Path(tempfile.mkdtemp(prefix='source-supplement-', dir=os.environ['FABRIC_SCRATCH_ROOT']))
    records, selected = {}, {}
    state = {'state': 'running', 'scratch': str(work), 'scratch_removed': False,
             'runner_sha256': digest(Path(__file__)), 'limits': {'file_bytes': FILE_LIMIT, 'group_bytes': GROUP_LIMIT}}
    durable_json(out / 'staging.json', state)
    try:
        total_selected = 0
        sequence = 0
        for identity, paths in requests.items():
            prior_id = identity.removesuffix('-current')
            prior = json.loads((BASE / prior_id / 'receipt.json').read_text())
            repository, revision = prior['repository'], prior['revision']
            record = {'repository': repository, 'revision': revision, 'coverage': 'selected files only', 'files': {}}
            if args.recovery_paths:
                prior = json.loads((BASE / 'supplements-01/receipt.json').read_text())[identity]
                revision = prior['revision']
                record.update(revision=revision, revision_source='supplements-01/receipt.json')
            elif identity.endswith('-current'):
                command = ['git', 'ls-remote', f'https://github.com/{repository}.git', 'HEAD']
                resolved = subprocess.run(command, capture_output=True, text=True, timeout=30)
                revision = resolved.stdout.split()[0] if resolved.returncode == 0 and resolved.stdout else ''
                if not re.fullmatch('[0-9a-f]{40}', revision):
                    raise RuntimeError('unable to resolve current immutable revision: ' + repository)
                record.update(revision=revision, historical_revision=prior['revision'],
                              resolve_command=command, resolve_exit=resolved.returncode, resolve_stdout=resolved.stdout)
            validate_pin(revision)
            records[identity] = record
            durable_json(out / 'receipt.json', records)
            for path in paths:
                source_name = PurePosixPath(path)
                if source_name.is_absolute() or '..' in source_name.parts or str(source_name) != path:
                    raise RuntimeError('unsafe supplement source path')
                if total_selected + FILE_LIMIT > GROUP_LIMIT:
                    raise RuntimeError('24MiB group cannot preserve another full8MiB failed prefix')
                sequence += 1
                target, error = work / f'{sequence:04}.source', work / f'{sequence:04}.stderr'
                url = f'https://raw.githubusercontent.com/{repository}/{revision}/{path}'
                record['files'][path] = {'state': 'running', 'url': url,
                                        'staged_source': str(target), 'staged_error': str(error)}
                durable_json(out / 'receipt.json', records)
                item = download_file(url, target, error)
                item['state'] = 'recorded'
                record['files'][path] = item
                # Stage files survive until the complete archive's exact readback.
                if item['bytes']:
                    selected[identity + '/' + path] = target
                total_selected += item['bytes']
                durable_json(out / 'receipt.json', records)
                if total_selected > GROUP_LIMIT:
                    raise RuntimeError('supplement decoded ceiling exceeded')
        archive = out / 'selected-source.tar.gz'
        with tarfile.open(archive, 'w:gz') as stream:
            for name, staged in selected.items():
                item = tarfile.TarInfo(name)
                item.size = staged.stat().st_size
                with staged.open('rb') as source:
                    stream.addfile(item, source)
        with archive.open('rb') as stream:
            os.fsync(stream.fileno())
        seen = archive_readback(archive, selected)
        (out / 'reader.py.gz').write_bytes(gzip.compress(Path(__file__).read_bytes(), mtime=0))
        summary = {'archive_sha256': digest(archive), 'readback_files': len(seen),
                   'failed_paths': [identity + '/' + path for identity, item in records.items()
                                    for path, row in item['files'].items() if row['exit']],
                   'scratch_removed': False, 'built_upstream': False}
        durable_json(out / 'summary.json', summary)
        shutil.rmtree(work)
        summary['scratch_removed'] = True
        durable_json(out / 'summary.json', summary)
        state.update(state='completed with recorded outcomes', scratch_removed=True)
        durable_json(out / 'staging.json', state)
        print(json.dumps(summary))
    except BaseException as error:
        state.update(state='interrupted' if isinstance(error, KeyboardInterrupt) else 'failed', error=repr(error),
                     staged_files=[p.name for p in work.iterdir()] if work.exists() else [],
                     scratch_removed=not work.exists())
        durable_json(out / 'staging.json', state)
        raise


if __name__ == '__main__':
    main()
