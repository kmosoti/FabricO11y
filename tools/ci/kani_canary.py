"""Require a working Kani proof and a specific reachable-assertion failure."""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from resource_group import STORAGE, require_limits

SOURCE = '''#[kani::proof]
fn canary_positive() {
    let x: u8 = kani::any();
    assert_eq!(x.wrapping_add(1).wrapping_sub(1), x);
}

#[kani::proof]
fn canary_negative() {
    assert!(false, "fabric_kani_negative_reachable");
}
'''


def main() -> int:
    require_limits()
    output = STORAGE / 'results/ci-tool-install'
    output.mkdir(parents=True, exist_ok=True)
    receipt = {'status': 'started', 'results': [], 'exit': None, 'cleanup_absent': False}
    status = 1
    try:
        with tempfile.TemporaryDirectory(
                prefix='kani-canary-', dir=os.environ['FABRIC_SCRATCH_ROOT']) as temporary:
            root = Path(temporary)
            (root / 'src').mkdir()
            (root / 'Cargo.toml').write_text(
                '[package]\nname="fabric-kani-canary"\nversion="0.0.0"\nedition="2021"\n\n[workspace]\n')
            (root / 'src/lib.rs').write_text(SOURCE)
            environment = dict(os.environ)
            environment['CARGO_TARGET_DIR'] = str(root / 'target')
            for harness in ('canary_positive', 'canary_negative'):
                command = ['cargo', 'kani', '--harness', harness]
                log = output / (harness + '.log')
                with log.open('w') as stream:
                    process = subprocess.run(command, cwd=root, env=environment,
                                             stdout=stream, stderr=subprocess.STDOUT, timeout=600)
                text = log.read_text()
                ran = 'Checking harness ' in text and harness in text
                expected_failure = (
                    process.returncode > 0
                    and 'Verification failed for - ' in text
                    and 'fabric_kani_negative_reachable' in text
                    and ran)
                accepted = (process.returncode == 0 and ran) if harness == 'canary_positive' else expected_failure
                receipt['results'].append({
                    'harness': harness, 'command': command, 'exit': process.returncode,
                    'accepted': accepted, 'log': str(log),
                })
                if not accepted:
                    raise RuntimeError('Kani canary did not produce its required semantic result: ' + harness)
        receipt['cleanup_absent'] = not root.exists()
        status = 0
        receipt['status'] = 'passed'
    except (OSError, KeyError, RuntimeError, subprocess.TimeoutExpired) as error:
        receipt['status'] = 'failed'
        receipt['error'] = str(error)
        print(str(error), file=sys.stderr)
    finally:
        receipt['exit'] = status
        (output / 'kani-canary.json').write_text(json.dumps(receipt, indent=2) + '\n')
    return status


if __name__ == '__main__':
    raise SystemExit(main())
