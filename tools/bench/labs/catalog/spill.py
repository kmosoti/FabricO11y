"""CR1 private codec screen; run only inside the resource launcher."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits, STORAGE


def dump(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def sha(path):
    with path.open('rb') as handle:
        return hashlib.file_digest(handle, 'sha256').hexdigest()


def exact(row, expected):
    return row['input_sha256'] == expected and row['output_sha256'] == expected and row['exact_bytes'] is True


def command(argv, stem, seconds=60):
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    began = time.monotonic()
    try:
        result = subprocess.run([str(x) for x in argv], capture_output=True, text=True, timeout=seconds)
    except subprocess.TimeoutExpired as error:
        stem.with_suffix('.stdout').write_bytes(error.stdout or b'')
        stem.with_suffix('.stderr').write_bytes(error.stderr or b'')
        dump(stem.with_suffix('.command.json'), {'argv': [str(x) for x in argv],
            'exit': None, 'stop_reason': 'timeout', 'limit_seconds': seconds,
            'whole_child_wall_s': time.monotonic() - began})
        raise
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    stem.with_suffix('.stdout').write_text(result.stdout)
    stem.with_suffix('.stderr').write_text(result.stderr)
    dump(stem.with_suffix('.command.json'), {
        'argv': [str(x) for x in argv], 'exit': result.returncode,
        'whole_child_wall_s': time.monotonic() - began,
        'whole_child_user_s': after.ru_utime - before.ru_utime,
        'whole_child_system_s': after.ru_stime - before.ru_stime,
        'whole_child_inblock': after.ru_inblock - before.ru_inblock,
        'whole_child_outblock': after.ru_oublock - before.ru_oublock,
        'boundary': 'includes startup and exact file readback; excludes fixture generation',
    })
    if result.returncode:
        raise RuntimeError('child failed: ' + str(stem))
    return json.loads(result.stdout)


def main():
    require_limits()
    parser = argparse.ArgumentParser()
    parser.add_argument('--plain', type=Path, required=True)
    parser.add_argument('--counted', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=False)
    good = {'input_sha256': 'fixture', 'output_sha256': 'fixture', 'exact_bytes': True}
    if not exact(good, 'fixture'):
        raise RuntimeError('unchanged exactness control rejected')
    for field in good:
        bad = dict(good, **{field: False if field == 'exact_bytes' else 'changed-byte-digest'})
        if exact(bad, 'fixture'):
            raise RuntimeError('exactness negative control accepted: ' + field)
    dump(args.out / 'exactness-controls.json', {'unchanged_accepted': True,
        'changed_input_rejected': True, 'changed_output_rejected': True, 'false_exactness_rejected': True})
    base = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True)
    if not base.is_relative_to(STORAGE / 'scratch'):
        raise RuntimeError('mounted data-drive scratch required')
    if shutil.disk_usage(base).free < 16 * 2**30:
        raise RuntimeError('16 GiB free-space reserve required')
    work = base / 'catalog-spill'
    work.mkdir()
    binaries = {'plain': args.plain.resolve(strict=True), 'counted': args.counted.resolve(strict=True)}
    hashes = {name: sha(binary) for name, binary in binaries.items()}
    dump(args.out / 'provenance.json', {
        'binaries': {name: str(binary) for name, binary in binaries.items()},
        'binary_sha256': hashes, 'seed': 2703163393, 'scratch': str(work),
        'source_sha256': {str(path.relative_to(ROOT)): sha(path) for path in (
            Path(__file__), ROOT / 'crates/fabric-server/examples/catalog_spill_probe.rs',
            ROOT / 'crates/fabric-server/src/segment/bounded/spill.rs',
            ROOT / 'docs/experiments/benchmarks/catalog-spill-protocol.md')},
        'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
        'affinity': sorted(os.sched_getaffinity(0)), 'cache': 'warm generated input; reused within screen',
    })
    try:
        for name, binary in binaries.items():
            controls = command([binary, 'controls'], args.out / (name + '-controls'), 120)
            if controls.get('controls') is not True:
                raise RuntimeError('control receipt absent')
        fixture = command([binaries['plain'], 'gen', work / 'input', '64'], args.out / 'fixture', 120)
        trials = []
        for name, binary in binaries.items():
            for candidate in ('encode', 'decode', 'combined'):
                for repetition in range(3):
                    order = ('baseline', candidate) if repetition % 2 == 0 else (candidate, 'baseline')
                    for variant in order:
                        stem = f'{name}-{candidate}-{repetition+1}-{variant}'
                        output = work / stem
                        row = command([binary, 'run', work / 'input', output, variant], args.out / stem)
                        if not exact(row, fixture['sha256']):
                            raise RuntimeError('fixture/roundtrip changed')
                        if (row['allocator'] is None) != (name == 'plain'):
                            raise RuntimeError('wrong plain/counted binary')
                        trials.append({'build': name, 'candidate': candidate, 'repetition': repetition+1, **row})
                        output.unlink()
        if any(sha(binary) != hashes[name] for name, binary in binaries.items()):
            raise RuntimeError('binary changed during screen')
        dump(args.out / 'result.json', {'trials': trials, 'exact_bytes': True,
            'microbenchmark_only': True, 'cpu_scope': 'whole child; counted timings diagnostic',
            'phase_scope': 'codec roundtrip + buffered output flush, excludes readback',
            'spool_ack_service_metrics': 'not applicable to isolated codec; composite required'})
        shutil.rmtree(work)
        dump(args.out / 'cleanup.json', {'removed': True})
    except BaseException as error:
        dump(args.out / 'failure.json', {'error': repr(error), 'retained_scratch': str(work)})
        raise


if __name__ == '__main__':
    main()
