"""Registered RC-GROUP screen using the unchanged C2 grader; not acceptance."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys

import builder


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', required=True, type=Path)
    args = parser.parse_args()
    builder.require_limits()
    args.out.mkdir(parents=True, exist_ok=False)
    work = Path(os.environ['FABRIC_SCRATCH_ROOT']) / 'chunk-groups-screen'
    work.mkdir(exist_ok=False)
    binary = Path(os.environ['CARGO_TARGET_DIR']) / 'release/examples/completion_builder'
    digest = builder.sha(binary)
    builder.write(args.out / 'provenance.json', {
        'binary': str(binary), 'binary_sha256': digest, 'argv': sys.argv,
        'scratch': str(work), 'screen_only': True,
        'checker_sha256': builder.sha(Path(builder.__file__)),
        'screen_sha256': builder.sha(Path(__file__)),
    })
    builder.write(args.out / 'controls.json', builder.controls())
    rows = {}
    try:
        for shape, mib in [('steady', 64), ('bigrows', 64), ('steady', 256)]:
            cell = f'{shape}-{mib}'
            out = args.out / cell
            out.mkdir()
            state = work / cell
            state.mkdir()
            fixture = builder.process([str(binary), 'gen', str(state / 'input'), shape, str(mib)],
                                      out / 'fixture', state, 300)
            pair = {}
            for arm in ('reference', 'bounded'):
                pair[arm] = builder.process([str(binary), 'run', str(state / arm), fixture['input'], arm],
                                           out / arm, state, 300)
            gates = builder.grade(pair['reference'], pair['bounded'], shape)
            rows[cell] = {'gates': gates, 'heap': pair['bounded']['incremental_peak_heap_bytes']}
            builder.write(out / 'pair.json', {**pair, 'gates': gates})
            if not all(gates.values()):
                raise RuntimeError(f'{cell}: {gates}')
            shutil.rmtree(state)
        scale = rows['steady-256']['heap'] * 10 <= rows['steady-64']['heap'] * 11
        builder.write(args.out / 'result.json', {'cells': rows, 'scale_within_10_percent': scale,
                                               'screen_passed': scale, 'acceptance': False})
        if not scale:
            raise RuntimeError('64 to 256 MiB heap scaling failed')
        if builder.sha(binary) != digest:
            raise RuntimeError('binary changed during screen')
        shutil.rmtree(work)
        builder.write(args.out / 'cleanup.json', {'removed': True, 'scratch': str(work)})
    except BaseException as error:
        builder.write(args.out / 'failure.json', {'error': repr(error), 'cells': rows,
                      'scratch': str(work), 'bytes': builder.footprint(work)})
        raise


if __name__ == '__main__':
    main()
