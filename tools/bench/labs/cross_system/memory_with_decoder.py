"""Install an isolated, pinned validation decoder, run the census, remove it."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

from source_fetch import require_limits


def main():
    require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('destination', type=Path)
    parser.add_argument('--reuse-census', type=Path)
    args = parser.parse_args()
    destination = args.destination.resolve()
    if destination.exists():
        raise RuntimeError('fresh evidence root required')
    dependency_record = destination.parent / (destination.name + '-decoder.json')
    if dependency_record.exists():
        raise RuntimeError('dependency receipt already exists')
    destination.parent.mkdir(parents=True, exist_ok=True)
    decoder = Path(os.environ['FABRIC_SCRATCH_ROOT']) / 'validation-decoder'
    decoder.mkdir()
    env = dict(os.environ, UV_PYTHON_DOWNLOADS='never', PYTHONDONTWRITEBYTECODE='1')
    command = ['uv', 'pip', 'install', '--no-cache', '--only-binary', ':all:',
               '--python', sys.executable, '--target', str(decoder), 'pyarrow==22.0.0']
    receipt = {'command': command, 'scope': 'independent Parquet validation only; no production dependency',
               'decoder': str(decoder)}
    try:
        result = subprocess.run(command, env=env, timeout=180)
        receipt['install_exit'] = result.returncode
        result.check_returncode()
        receipt['package_metadata'] = {
            str(p.relative_to(decoder)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in decoder.glob('*.dist-info/*') if p.is_file()}
        env['PYTHONPATH'] = str(decoder)
        dependency_record.write_text(json.dumps(receipt, indent=2) + '\n')
        command = [sys.executable, '-B', 'tools/bench/labs/cross_system/memory_census.py',
                   '--destination', str(destination), '--protocol',
                   'docs/experiments/benchmarks/cross-system-continuation-protocol.md',
                   '--seed', '2703204353']
        if args.reuse_census:
            command += ['--reuse-census', str(args.reuse_census.resolve())]
        receipt['census_command'] = command
        result = subprocess.run(command, env=env, timeout=650)
        receipt['census_exit'] = result.returncode
        result.check_returncode()
    finally:
        # This is replaceable pinned tooling, never a native fixture or failure state.
        shutil.rmtree(decoder)
        receipt['decoder_removed'] = not decoder.exists()
        dependency_record.write_text(json.dumps(receipt, indent=2) + '\n')


if __name__ == '__main__':
    main()
