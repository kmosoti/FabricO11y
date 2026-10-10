#!/usr/bin/env python3
"""Execute only the preregistered encoded-page diagnostic, preserving receipts."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import STORAGE, require_limits
import builder


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def main():
    group = require_limits()
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--build-command', required=True)
    args = parser.parse_args()
    out = args.out.absolute()
    if out.exists() or not out.parent.resolve().is_relative_to(STORAGE.resolve()):
        raise RuntimeError('fresh external data-drive result path required')
    census = subprocess.run(['du', '-sx', '-B1', str(STORAGE)], capture_output=True, text=True, check=True)
    occupied = int(census.stdout.split()[0])
    if occupied + 2 * 1024**3 > 100_000_000_000:
        raise RuntimeError('100 GB storage allowance cannot admit probe reserve')
    out.mkdir()
    binary = out / 'completion-builder-frozen'
    shutil.copy2(args.binary, binary)
    members = [ROOT / 'Cargo.lock', ROOT / 'crates/fabric-server/Cargo.toml',
               ROOT / 'crates/fabric-server/examples/completion_builder.rs',
               ROOT / 'crates/fabric-server/src/segment.rs',
               ROOT / 'crates/fabric-server/src/segment/bounded.rs',
               ROOT / 'crates/fabric-server/src/text_filter.rs',
               ROOT / 'docs/experiments/formal/encoded-page-memory-protocol.md',
               Path(__file__).resolve(), ROOT / 'tools/bench/labs/completion/builder.py']
    members += sorted((ROOT / 'crates/fabric-server/src/segment/bounded').glob('*.rs'))
    frozen = out / 'sources'
    for source in members:
        destination = frozen / source.relative_to(ROOT)
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
    hashes = {str(path.relative_to(ROOT)): sha(path) for path in members}
    copied_hashes = {str(path.relative_to(ROOT)): sha(frozen / path.relative_to(ROOT)) for path in members}
    if copied_hashes != hashes:
        raise RuntimeError('frozen source copies differ')
    provenance = {'argv': sys.argv, 'build_command': args.build_command,
                  'binary_sha256': sha(binary), 'sources_sha256': hashes,
                  'copied_sources_sha256': copied_hashes,
                  'cgroup': str(group), 'storage_bytes_before': occupied,
                  'storage_census': census.stdout, 'seed': 0xA11FA001,
                  'rows': 8192, 'body_bytes': 16384, 'comparison': 'diagnostic single pair'}
    write(out / 'provenance.json', provenance)
    work = Path(os.environ['FABRIC_SCRATCH_ROOT']) / out.name
    work.mkdir()
    environment = {key: value for key, value in os.environ.items() if not key.startswith('FABRIC_FAULT_')}

    def child(label, arguments):
        command = [str(binary), *map(str, arguments)]
        if sha(binary) != provenance['binary_sha256']:
            raise RuntimeError('frozen binary changed before child')
        result = subprocess.run(command, capture_output=True, text=True, env=environment, timeout=300)
        receipt = {'command': command, 'exit': result.returncode,
                   'stdout': result.stdout, 'stderr': result.stderr,
                   'binary_hash_after': sha(binary)}
        write(out / (label + '-command.json'), receipt)
        if receipt['binary_hash_after'] != provenance['binary_sha256'] or result.returncode:
            raise RuntimeError('child failed or frozen binary changed: ' + label)
        return json.loads(result.stdout.splitlines()[-1])

    fixture = child('generate', ['entropy-gen', work / 'fixture', 8192])
    write(out / 'fixture.json', fixture)
    input_path = Path(fixture['input'])
    a = child('reference', ['run', work / 'reference', input_path, 'reference'])
    write(out / 'reference.json', a)
    b = child('bounded', ['run', work / 'bounded', input_path, 'bounded'])
    write(out / 'bounded.json', b)
    checks = builder.grade(a, b, 'entropy')
    checks['exact_fixture_counts'] = a['logical_row_counts'] == b['logical_row_counts'] == {
        'logs': 8192, 'metrics': 0, 'gaps': 0, 'batches': 256}
    checks['one_full_log_group'] = a['pruning']['logs']['group_rows'] == b['pruning']['logs']['group_rows'] == [8192]
    checks['exact_filter_bytes'] = a['manifest']['files']['text_filter.bin'] == b['manifest']['files']['text_filter.bin']
    checks['counted_allocator'] = a['allocator_counted'] and b['allocator_counted']
    checks['frozen_sources_unchanged'] = all(sha(ROOT / path) == value for path, value in hashes.items())
    checks['input_unchanged'] = sha(input_path) == fixture['input_sha256'] == a['input_sha256'] == b['input_sha256']
    write(out / 'pair.json', {'reference': a, 'bounded': b, 'checks': checks,
                            'all_checks': all(checks.values())})
    # A heap counterexample is the admitted observation, not a harness failure.
    valid = all(value for key, value in checks.items() if key != 'heap_ceiling')
    if not valid:
        write(out / 'cleanup.json', {'removed': False, 'retained_scratch': str(work)})
        raise RuntimeError('diagnostic correctness or provenance failed')
    shutil.rmtree(work)
    write(out / 'cleanup.json', {'removed': not work.exists(), 'owned_scratch': str(work)})
    print(json.dumps({'result': str(out), 'heap_ceiling': checks['heap_ceiling'],
                      'bounded_incremental_peak': b['incremental_peak_heap_bytes'],
                      'correctness': valid, 'cleanup': not work.exists()}))


if __name__ == '__main__':
    main()
