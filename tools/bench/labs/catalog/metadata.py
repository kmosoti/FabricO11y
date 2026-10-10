#!/usr/bin/env python3
"""CR3 fixed-work metadata ownership screen; launch only under resource_group."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import STORAGE, require_limits

CAP = 64 * 1024**2
FREE = 16 * 1024**3


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def dump(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def size(path):
    return sum(p.stat().st_size for p in path.rglob('*') if p.is_file())


def run(argv, output, timeout, work, evidence):
    if size(work) > CAP or size(evidence) > CAP or shutil.disk_usage(STORAGE).free < FREE:
        raise RuntimeError('64MiB scratch/evidence ceiling or 16GiB reserve exceeded')
    began = time.monotonic()
    failure = None
    with output.open('wb') as stdout, output.with_suffix('.stderr').open('wb') as stderr:
        child = subprocess.Popen(argv, cwd=ROOT, stdout=stdout, stderr=stderr,
                                 env=dict(os.environ), start_new_session=True)
        try:
            while child.poll() is None:
                if time.monotonic() - began > timeout:
                    raise TimeoutError(f'child exceeded {timeout}s: {argv}')
                if size(work) > CAP or size(evidence) > CAP:
                    raise RuntimeError('64MiB scratch/evidence ceiling exceeded')
                if shutil.disk_usage(STORAGE).free < FREE:
                    raise RuntimeError('16GiB reserve exhausted')
                time.sleep(.1)
        except BaseException as error:
            failure = error
        finally:
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=10)
    receipt = {'argv': argv, 'exit': child.returncode,
               'elapsed_seconds': time.monotonic() - began,
               'stdout_sha256': sha(output), 'stderr_sha256': sha(output.with_suffix('.stderr')),
               'failure': repr(failure) if failure is not None else None}
    dump(output.with_suffix('.command.json'), receipt)
    if failure is not None:
        raise failure
    if child.returncode:
        raise RuntimeError(f'child exit {child.returncode}: {argv}')
    return receipt


def check(result, mode, readers, counted):
    expected = {'stage': 'complete', 'mode': mode, 'readers': readers, 'seed': 42,
                'segments': 64, 'nodes_per_manifest': 20, 'total_operations': 4096,
                'manifest_acquisitions': 262144, 'allocator_counted': counted}
    for key, value in expected.items():
        if result.get(key) != value:
            raise RuntimeError(f'native receipt drift: {key}')
    controls = result['controls']
    if controls.get('exact_manifest_equality') is not True or len(controls['rejected_mutations']) != 5 or set(controls['rejected_mutations']) != {
            'group_bound', 'freshness', 'file_digest', 'file_rows', 'missing_file'}:
        raise RuntimeError('exact equality/mutation controls incomplete')
    life = result['lifecycle']
    for key in ('logical_expiration_control', 'stale_handle_authorization_rejected',
                'all_manifests_released_after_cancel'):
        if life.get(key) is not True:
            raise RuntimeError('lifecycle control failed: ' + key)
    if life['generations'] != 8 or life['paused_handles'] != 512:
        raise RuntimeError('generation/handle accounting drift')
    if len(life['logical_serialized_bytes_by_generation']) != 8:
        raise RuntimeError('retained generation byte accounting incomplete')
    if counted and result['allocation_after'][3] <= 0:
        raise RuntimeError('counted allocator missing')
    if not counted and (result['allocation_before'] != [0] * 4 or
                        result['allocation_after'] != [0] * 4):
        raise RuntimeError('plain binary contains allocation observer')


def checker_controls(result, mode, readers, counted):
    rejected = []
    for name in ('missing_operation', 'wrong_mode', 'missing_mutation', 'stale_snapshot', 'leaked_generation'):
        bad = copy.deepcopy(result)
        if name == 'missing_operation':
            bad['total_operations'] -= 1
        elif name == 'wrong_mode':
            bad['mode'] = 'invalid'
        elif name == 'missing_mutation':
            bad['controls']['rejected_mutations'].pop()
        elif name == 'stale_snapshot':
            bad['lifecycle']['logical_expiration_control'] = False
        else:
            bad['lifecycle']['all_manifests_released_after_cancel'] = False
        try:
            check(bad, mode, readers, counted)
        except RuntimeError:
            rejected.append(name)
        else:
            raise RuntimeError('receipt checker accepted injected defect: ' + name)
    return rejected


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--plain', type=Path)
    parser.add_argument('--counted', type=Path)
    args = parser.parse_args()
    if (args.plain is None) != (args.counted is None):
        parser.error('--plain and --counted must be supplied together')
    require_limits()
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    if not scratch.is_relative_to(STORAGE / 'scratch'):
        raise RuntimeError('launcher-owned data-drive scratch required')
    evidence = args.out.resolve()
    bases = (ROOT / 'docs/experiments/benchmarks/data/catalog-labs-run-01/query',
             ROOT / 'docs/experiments/benchmarks/data/catalog-metadata-run-01')
    if not any(evidence.is_relative_to(base) for base in bases) or evidence.exists():
        raise RuntimeError('fresh catalog query evidence subdirectory required')
    work = scratch / 'catalog-metadata'
    if work.exists():
        raise RuntimeError('scratch already exists')
    evidence.mkdir(parents=True)
    work.mkdir()
    (work / 'owned').write_text(str(evidence))
    status = 'interrupted'
    began = time.monotonic()
    rows = []
    try:
        source = ROOT / 'crates/fabric-server/examples/catalog_metadata_probe.rs'
        protocol = ROOT / 'docs/experiments/benchmarks/catalog-metadata-protocol.md'
        for path in (source, protocol, Path(__file__)):
            # Frozen protocol bytes are evidence, not a relocated live Markdown
            # page whose original relative links should be resolved here.
            name = path.stem + '.txt' if path.suffix == '.md' else path.name
            shutil.copy2(path, evidence / name)
        dump(evidence / 'environment.json', {
            'argv': sys.argv, 'uname': list(os.uname()), 'affinity': sorted(os.sched_getaffinity(0)),
            'scratch': str(work), 'seed': 42, 'native_sha256': sha(source),
            'harness_sha256': sha(Path(__file__)), 'protocol_sha256': sha(protocol),
            'manifest_source_sha256': sha(ROOT / 'crates/fabric-server/src/segment.rs'),
            'core_query_sha256': sha(ROOT / 'crates/fabric-core/src/query.rs'),
            'build_settings': {k: os.environ.get(k) for k in ('RUSTFLAGS', 'CARGO_TARGET_DIR', 'CARGO_BUILD_JOBS')},
            'limits': {'scratch_bytes': CAP, 'evidence_bytes': CAP, 'reserve_bytes': FREE,
                       'build_timeout_seconds': 240, 'trial_timeout_seconds': 60}})
        binaries = {}
        for variant in ('plain', 'counted'):
            frozen = getattr(args, variant)
            if frozen is None:
                argv = ['cargo', 'build', '--offline', '--locked', '--release', '-p', 'fabric-server',
                        '--example', 'catalog_metadata_probe']
                if variant == 'counted':
                    argv += ['--features', 'responsibility-alloc-probe']
                run(argv, evidence / f'{variant}-build.stdout', 240, work, evidence)
                frozen = Path(os.environ['CARGO_TARGET_DIR']) / 'release/examples/catalog_metadata_probe'
            else:
                frozen = frozen.resolve()
                if not frozen.is_relative_to(STORAGE):
                    raise RuntimeError('frozen binaries must reside on required data drive')
            binary = work / variant
            shutil.copy2(frozen, binary)
            binaries[variant] = sha(binary)
        dump(evidence / 'binaries.json', binaries)
        identity = None
        for variant in ('plain', 'counted'):
            for readers in (1, 4):
                for pair in (1, 2, 3):
                    for mode in (('clone', 'arc') if pair % 2 else ('arc', 'clone')):
                        if time.monotonic() - began > 1440:
                            raise TimeoutError('24-minute aggregate metadata allowance exhausted')
                        output = evidence / f'{variant}-r{readers}-p{pair}-{mode}.json'
                        receipt = run([str(work / variant), mode, str(readers)], output, 60, work, evidence)
                        result = json.loads(output.read_text())
                        check(result, mode, readers, variant == 'counted')
                        rejected = checker_controls(result, mode, readers, variant == 'counted')
                        current = (result['fixture_sha256'], result['checksum'],
                                   result['logical_serialized_fixture_bytes'])
                        if identity is not None and identity != current:
                            raise RuntimeError('fixture/work identity mismatch across matrix')
                        identity = current
                        rows.append({'variant': variant, 'readers': readers, 'pair': pair, 'mode': mode,
                                     'output': output.name, 'sha256': sha(output), 'receipt': receipt,
                                     'wall_ns': result['wall_ns'], 'allocation_after': result['allocation_after'],
                                     'mutation_controls': 5, 'lifecycle_controls': 3,
                                     'receipt_checker_rejected': rejected})
                        dump(evidence / 'trials.json', rows)
        if len(rows) != 24:
            raise RuntimeError('incomplete fixed matrix')
        status = 'completed'
    except BaseException as error:
        status = 'interrupted' if isinstance(error, (KeyboardInterrupt, TimeoutError)) else 'failed'
        dump(evidence / 'failure.json', {'status': status, 'error': repr(error), 'scratch_preserved': str(work)})
        raise
    finally:
        scratch_bytes = size(work)
        if status == 'completed':
            if (work / 'owned').read_text() != str(evidence):
                raise RuntimeError('scratch ownership mismatch')
            shutil.rmtree(work)
        dump(evidence / 'cleanup.json', {'status': status, 'removed': not work.exists(),
             'scratch': str(work), 'scratch_bytes': scratch_bytes, 'evidence_bytes': size(evidence),
             'elapsed_seconds': time.monotonic() - began, 'trials_completed': len(rows)})
    print(json.dumps({'status': status, 'out': str(evidence), 'trials': len(rows)}))


if __name__ == '__main__':
    main()
