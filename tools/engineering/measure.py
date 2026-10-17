#!/usr/bin/env python3
"""Finite paired measurements registered in engineering-evidence-protocol.md."""
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import statistics
import sys
import tempfile
import time
import tracemalloc

from evidence import ArtifactStore, ordered_jobs


def scan_reference(jobs):
    remaining = {job['id']: set(job['needs']) for job in jobs}
    emitted, output = set(), []
    while remaining:
        ready = sorted(name for name, needs in remaining.items() if needs <= emitted)
        if not ready:
            raise ValueError('cycle')
        name = ready[0]
        emitted.add(name)
        output.append(name)
        del remaining[name]
    return output


def observed(call):
    start = time.perf_counter_ns()
    answer = call()
    return answer, time.perf_counter_ns() - start


def allocations(call):
    tracemalloc.start()
    try:
        call()
        return tracemalloc.get_traced_memory()[1]
    finally:
        tracemalloc.stop()


def paired(reference, indexed):
    expected = reference()  # one warmup for each implementation
    assert indexed() == expected
    samples = {'reference_ns': [], 'indexed_ns': []}
    for pair in range(7):
        calls = [('reference_ns', reference), ('indexed_ns', indexed)]
        if pair % 2:
            calls.reverse()
        for key, call in calls:
            answer, elapsed = observed(call)
            assert answer == expected, 'paired output changed'
            samples[key].append(elapsed)
    return {**samples, 'reference_median_ns': statistics.median(samples['reference_ns']),
            'indexed_median_ns': statistics.median(samples['indexed_ns']),
            'reference_peak_python_bytes': allocations(reference),
            'indexed_peak_python_bytes': allocations(indexed), 'exact_output_equal': True}


def main():
    # The entrypoint refuses uncapped execution; test helpers are pure functions.
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
    from resource_group import require_limits
    require_limits()
    report = {'protocol': 'docs/experiments/formal/engineering-evidence-protocol.md',
              'seed': 17, 'python': sys.version, 'platform': platform.platform(),
              'cpus': os.cpu_count(), 'graphs': [], 'scope': 'finite tooling costs; no product or security qualification'}
    for size in (64, 512, 2048):
        for shape in ('chain', 'fan-out'):
            jobs = [{'id': f'j{i:04d}', 'needs': [] if i == 0 else
                     [f'j{i - 1:04d}' if shape == 'chain' else 'j0000']}
                    for i in range(size)]
            random.Random(17).shuffle(jobs)
            report['graphs'].append({'size': size, 'edges': size - 1, 'shape': shape,
                                     **paired(lambda: scan_reference(jobs), lambda: ordered_jobs(jobs))})
    with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT'], prefix='evidence-measure-') as name:
        root = Path(name)
        fixture = root / 'artifact.bin'
        fixture.write_bytes(bytes(range(256)) * 32768)  # exactly 8 MiB
        expected = hashlib.sha256(fixture.read_bytes()).hexdigest()

        def repeated():
            answers = []
            for _ in range(32):
                with fixture.open('rb') as stream:
                    answers.append(hashlib.file_digest(stream, 'sha256').hexdigest())
            assert all(answer == expected for answer in answers)
            return answers

        def composed():
            store = ArtifactStore(root)
            try:
                for _ in range(32):
                    store.read('artifact.bin', expected)
                store.recheck()
                assert store.bytes_hashed == 8 * 1024**2
                return [expected] * 32
            finally:
                store.close()

        report['artifact'] = {'bytes': 8 * 1024**2, 'joins': 32,
                              'reference_bytes_hashed': 32 * 8 * 1024**2,
                              'indexed_bytes_hashed': 8 * 1024**2,
                              **paired(repeated, composed)}
    print(json.dumps(report, sort_keys=True, indent=2))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
