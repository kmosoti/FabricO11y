"""Run the admitted RC-GROUP repetitions against a frozen counted binary."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

import builder


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', required=True, type=Path)
    parser.add_argument('--screen', required=True, type=Path)
    args = parser.parse_args()
    builder.require_limits()
    original = Path(os.environ['CARGO_TARGET_DIR']) / 'release/examples/completion_builder'
    screen = json.loads((args.screen / 'result.json').read_text())
    admitted_hash = json.loads((args.screen / 'provenance.json').read_text())['binary_sha256']
    if not screen['screen_passed'] or builder.sha(original) != admitted_hash:
        raise RuntimeError('passing screen with identical binary required')
    args.out.mkdir(parents=True, exist_ok=False)
    frozen = args.out / 'frozen'
    frozen.mkdir()
    binary = frozen / 'completion_builder'
    shutil.copy2(original, binary)
    sources = [Path(builder.__file__), Path(__file__),
               builder.ROOT / 'crates/fabric-server/examples/completion_builder.rs',
               builder.ROOT / 'crates/fabric-server/src/segment.rs',
               builder.ROOT / 'crates/fabric-server/src/segment/bounded.rs',
               builder.ROOT / 'crates/fabric-server/src/text_filter.rs',
               builder.ROOT / 'Cargo.lock']
    sources += list((builder.ROOT / 'crates/fabric-server/src/segment/bounded').rglob('*.rs'))
    hashes = {}
    for source in sources:
        relative = source.relative_to(builder.ROOT)
        target = frozen / 'source' / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)
        hashes[str(relative)] = builder.sha(target)
        if builder.sha(source) != hashes[str(relative)]:
            raise RuntimeError('source changed during freeze')
    builder.write(args.out / 'freeze.json', {'binary_sha256': builder.sha(binary), 'sources': hashes,
                  'screen': str(args.screen), 'argv': sys.argv})
    for shape, mib in builder.CELLS:
        cell = f'{shape}-{mib}'
        command = [sys.executable, '-B', str(Path(builder.__file__)), '--cell', cell,
                   '--out', str(args.out / ('builder-' + cell)), '--binary', str(binary),
                   '--process-seconds', '300']
        if cell == 'steady-256':
            command += ['--baseline', str(args.out / 'builder-steady-64/result.json')]
        print('START ' + cell, flush=True)
        result = subprocess.run(command)
        builder.write(args.out / (cell + '-command.json'), {'argv': command, 'exit': result.returncode})
        if result.returncode:
            raise RuntimeError(f'{cell} failed; dependent cells stopped')
        if builder.sha(binary) != admitted_hash:
            raise RuntimeError('frozen binary changed')
        print('COMPLETE ' + cell, flush=True)
    builder.write(args.out / 'result.json', {'repeated_builder_gates_passed': True,
                  'cells': len(builder.CELLS), 'pairs_per_cell': 3, 'full_bs_acceptance': False,
                  'spill_observer_pending': True, 'soak_pending': True})


if __name__ == '__main__':
    main()
