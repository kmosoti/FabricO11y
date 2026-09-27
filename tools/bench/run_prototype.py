#!/usr/bin/env python3
"""Reproduce the local research lifecycle in separate processes; preserve evidence."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sync_dir(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def fresh_json(path, value):
    with path.open('x') as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())
    sync_dir(path.parent)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--repo-root', type=Path, default=ROOT)
    args = parser.parse_args()
    repo, output = args.repo_root.resolve(), args.output.resolve()
    output.mkdir(exist_ok=False)  # Existing parent required; never replace evidence.
    sync_dir(output)
    sync_dir(output.parent)
    commands = []

    def invoke(command, cwd, expected=0):
        start = time.perf_counter_ns()
        result = subprocess.run(list(map(str, command)), cwd=cwd, capture_output=True, text=True)
        record = {'command': list(map(str, command)), 'cwd': str(cwd),
                  'exit_code': result.returncode, 'expected_exit': expected,
                  'elapsed_ns': time.perf_counter_ns() - start,
                  'stdout': result.stdout, 'stderr': result.stderr}
        commands.append(record)
        fresh_json(output / f'command-{len(commands):02}.json', record)
        if result.returncode != expected:
            raise RuntimeError(f'command {len(commands)}: exit {result.returncode}, wanted {expected}')
        return json.loads(result.stdout) if expected == 0 and result.stdout else None

    invoke(['cargo', 'build', '--release', '--offline', '--locked', '--manifest-path',
            'tools/storage-probe/Cargo.toml', '--bin', 'fabric-research'], repo)
    binary = repo / 'tools/storage-probe/target/release/fabric-research'

    def call(*args, expected=0):
        return invoke([binary, *args], output, expected)

    def answer(report, complete, positions):
        if report.get('complete') is not complete or report.get('positions') != positions:
            raise RuntimeError(f'wrong answer: {report}')
        checkpoint = output / report['checkpoint']
        if sha(checkpoint) != report['checkpoint_sha256']:
            raise RuntimeError('checkpoint digest differs from retained report')

    records = [{'timeUnixNano': str(t), 'body': {'stringValue': body}}
               for t, body in [(10, 'rare first'), (5, 'ordinary'), (10, 'rare\u2003last')]]
    request = {'resourceLogs': [{'scopeLogs': [{'logRecords': records}]}]}
    fresh_json(output / 'request.json', request)
    fresh_json(output / 'source-config.json', {'tenant': 7, 'source': 8, 'resource': 9, 'first_event_id': 100})
    fresh_json(output / 'query.json', {'start_ns': 0, 'end_ns': 20, 'tenant': 7, 'token': 'rare'})
    original_request_hash = sha(output / 'request.json')
    call('adapt-otlp', 'request.json', 'source-config.json', 'input.json')
    readiness_start = time.perf_counter_ns()
    ingested = call('ingest', 'input.json', 'events.fol', '1', '2')
    if ingested['appended'] != 3 or ingested['peak_buffer_events'] > 1:
        raise RuntimeError('bounded ingestion lost events')
    call('publish', 'events.fol', 'snapshot', 'trusted.json', '1', '41')
    initial = call('query', 'snapshot', 'trusted.json', 'query.json', 'all', 'initial.json')
    answer(initial, True, [0, 2])
    readiness_ns = time.perf_counter_ns() - readiness_start
    call('verify', 'input.json', 'trusted.json', 'query.json', 'initial.json', initial['checkpoint_sha256'])
    original_log_hash = sha(output / 'events.fol')
    retry = call('ingest', 'input.json', 'events.fol', '1', '2')
    if retry['appended'] != 0 or sha(output / 'events.fol') != original_log_hash:
        raise RuntimeError('identical retry changed the log')

    first = call('query', 'snapshot', 'trusted.json', 'query.json', 'none', 'partial.json')
    answer(first, False, [])
    snapshot = output / 'snapshot'
    (snapshot / 'cold').mkdir()
    (snapshot / 'block-0.json').rename(snapshot / 'cold/block-0.json')
    # Retain the unavailable block outside the snapshot, then restore it later.
    (snapshot / 'block-2.json').rename(output / 'retained-block-2.json')
    for directory in [snapshot / 'cold', snapshot, output]:
        sync_dir(directory)
    second = call('resume', 'snapshot', 'trusted.json', 'query.json', 'partial.json',
                  first['checkpoint_sha256'], 'all', 'still-partial.json')
    answer(second, False, [0])
    call('verify', 'input.json', 'trusted.json', 'query.json', 'still-partial.json',
         second['checkpoint_sha256'], expected=1)
    (output / 'retained-block-2.json').rename(snapshot / 'block-2.json')
    sync_dir(snapshot)
    sync_dir(output)
    final = call('resume', 'snapshot', 'trusted.json', 'query.json', 'still-partial.json',
                 second['checkpoint_sha256'], 'all', 'complete.json')
    answer(final, True, [0, 2])
    call('verify', 'input.json', 'trusted.json', 'query.json', 'complete.json', final['checkpoint_sha256'])

    (snapshot / 'manifest.json').rename(output / 'retained-manifest.json')
    call('query', 'snapshot', 'trusted.json', 'query.json', 'all', 'must-not-exist.json', expected=1)
    call('rebuild', 'snapshot', 'trusted.json')
    rebuilt = call('query', 'snapshot', 'trusted.json', 'query.json', 'all', 'rebuilt-query.json')
    answer(rebuilt, True, [0, 2])

    records.append({'timeUnixNano': '1', 'body': {'stringValue': 'rare late arrival'}})
    fresh_json(output / 'successor-request.json', request)
    call('adapt-otlp', 'successor-request.json', 'source-config.json', 'successor-input.json')
    suffix = call('ingest', 'successor-input.json', 'events.fol', '1', '2')
    if suffix['appended'] != 1 or suffix['already_committed'] != 3:
        raise RuntimeError('successor did not preserve the committed prefix')
    call('publish', 'events.fol', 'successor', 'successor-trusted.json', '1', '42')
    call('resume', 'successor', 'successor-trusted.json', 'query.json', 'complete.json',
         final['checkpoint_sha256'], 'all', 'wrong-snapshot.json', expected=1)
    latest = call('query', 'successor', 'successor-trusted.json', 'query.json', 'all', 'latest.json')
    answer(latest, True, [0, 2, 3])
    call('verify', 'successor-input.json', 'successor-trusted.json', 'query.json', 'latest.json', latest['checkpoint_sha256'])
    if sha(output / 'request.json') != original_request_hash:
        raise RuntimeError('source was changed')
    if (output / 'must-not-exist.json').exists() or (output / 'wrong-snapshot.json').exists():
        raise RuntimeError('rejected operation wrote output')
    fresh_json(output / 'result.json', {
        'status': 'passed', 'commands': len(commands), 'first_positions': [0, 2],
        'successor_positions': [0, 2, 3], 'binary_sha256': sha(binary),
        'durable_ingest_to_first_complete_query_ns': readiness_ns,
        'timing_limit': 'One illustrative process lifecycle on a shared host, including process launch and report checks; not a latency benchmark or device power-loss test.',
        'source_sha256': {str(p.relative_to(repo)): sha(p) for directory in ['src', 'tools/storage-probe/src']
                          for p in (repo / directory).rglob('*.rs')},
    })
    fresh_json(output / 'SHA256SUMS.json', {str(p.relative_to(output)): sha(p)
               for p in sorted(output.rglob('*')) if p.is_file()})
    print(f'Prototype lifecycle passed; {len(commands)} process commands preserved in {output}')


if __name__ == '__main__':
    main()
