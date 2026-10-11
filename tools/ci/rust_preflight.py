"""Validate the contained Rust toolchain and disable implicit manager self-update."""
from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
from collections.abc import Mapping

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from resource_group import STORAGE, require_limits


def homes(environment: Mapping[str, str], storage: Path) -> dict[str, str]:
    root = (storage / 'toolchain-cache').resolve(strict=True)
    result = {}
    for name in ('CARGO_HOME', 'RUSTUP_HOME'):
        value = environment.get(name)
        if not value or not Path(value).is_absolute():
            raise ValueError(f'{name} must explicitly name an absolute directory')
        path = Path(value).resolve(strict=True)
        if not path.is_dir() or path == root or not path.is_relative_to(root):
            raise ValueError(f'{name} must remain inside mounted toolchain storage')
        result[name] = str(path)
    if result['CARGO_HOME'] == result['RUSTUP_HOME']:
        raise ValueError('Cargo and rustup homes must be distinct')
    return result


def invoke(command: list[str], receipt: dict) -> str:
    process = subprocess.run(command, capture_output=True, text=True, timeout=120)
    receipt['commands'].append({
        'command': command, 'exit': process.returncode,
        'stdout': process.stdout[-8192:], 'stderr': process.stderr[-8192:],
    })
    if process.returncode != 0:
        raise RuntimeError('toolchain preflight command failed: ' + command[0])
    return process.stdout.strip()


def main() -> int:
    require_limits()
    receipt = {'status': 'started', 'commands': [], 'exit': None}
    directory = STORAGE / 'results/ci-tool-install'
    directory.mkdir(parents=True, exist_ok=True)
    status = 1
    try:
        receipt['homes'] = homes(os.environ, STORAGE)
        receipt['launchers'] = {}
        for name in ('rustup', 'cargo', 'rustc'):
            executable = shutil.which(name)
            if executable is None:
                raise RuntimeError('missing toolchain executable: ' + name)
            receipt['launchers'][name] = str(Path(executable).resolve(strict=True))
        # The resource launcher intentionally does not import the entire CI
        # environment. Configure rustup explicitly rather than depending on CI
        # detection to suppress self-update against a relocated Cargo home.
        invoke(['rustup', 'set', 'auto-self-update', 'disable'], receipt)
        receipt['source_commit'] = invoke(['git', 'rev-parse', 'HEAD'], receipt)
        invoke(['rustup', '--version'], receipt)
        toolchain_root = Path(receipt['homes']['RUSTUP_HOME']) / 'toolchains'
        for name in ('cargo', 'rustc'):
            selected = Path(invoke(['rustup', 'which', name], receipt)).resolve(strict=True)
            if not selected.is_file() or not selected.is_relative_to(toolchain_root):
                raise RuntimeError('selected toolchain escapes mounted storage: ' + name)
            version = invoke([name, '--version'], receipt)
            if not version.startswith(name + ' 1.99.0 '):
                raise RuntimeError('unexpected pinned toolchain version: ' + name)
        status = 0
        receipt['status'] = 'passed'
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        receipt['status'] = 'failed'
        receipt['error'] = str(error)
        print('Rust preflight failed: ' + str(error), file=sys.stderr)
    finally:
        receipt['exit'] = status
        temporary = directory / 'rust-preflight.pending'
        temporary.write_text(json.dumps(receipt, indent=2) + '\n')
        temporary.replace(directory / 'rust-preflight.json')
    return status


if __name__ == '__main__':
    raise SystemExit(main())
