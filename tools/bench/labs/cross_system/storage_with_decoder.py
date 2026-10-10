"""Pinned ephemeral decoder around one finite storage sweep; launcher required."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

import memory_census as mc
from storage_sweep import REGISTERED


def main():
    mc.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--destination', type=Path, required=True)
    parser.add_argument('--protocol', type=Path, required=True)
    parser.add_argument('--build-manifest', type=Path, required=True)
    parser.add_argument('--target-mib', type=int, choices=[16, 64], required=True)
    parser.add_argument('--repeat', type=int, choices=[0, 1], required=True)
    args = parser.parse_args()
    destination = args.destination.resolve()
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    if not scratch.is_relative_to(mc.DATA.resolve()) or not (
            destination.is_relative_to(REGISTERED.resolve()) or destination.is_relative_to(mc.DATA.resolve())):
        raise RuntimeError('registered/data-drive evidence and data-drive scratch required')
    receipt_path = destination.parent / (destination.name + '-decoder.json')
    if destination.exists() or receipt_path.exists():
        raise RuntimeError('fresh sweep/decoder receipt required')
    destination.parent.mkdir(parents=True, exist_ok=True)
    decoder = Path(os.environ['FABRIC_SCRATCH_ROOT']) / 'storage-validation-decoder'
    decoder.mkdir()
    env = dict(os.environ, UV_PYTHON_DOWNLOADS='never', PYTHONDONTWRITEBYTECODE='1')
    command = ['uv', 'pip', 'install', '--no-cache', '--only-binary', ':all:',
               '--python', sys.executable, '--target', str(decoder), 'pyarrow==22.0.0']
    receipt = {'install_command': command, 'decoder': str(decoder), 'scope': 'validation only'}
    try:
        result = subprocess.run(command, env=env, timeout=45)
        receipt['install_exit_code'] = result.returncode
        result.check_returncode()
        receipt['package_metadata_sha256'] = {str(path.relative_to(decoder)): mc.digest(path)
                    for path in decoder.glob('*.dist-info/*') if path.is_file()}
        env['PYTHONPATH'] = str(decoder)
        command = [sys.executable, '-B', str(Path(__file__).with_name('storage_sweep.py')), 'run',
                   '--destination', str(destination), '--protocol', str(args.protocol),
                   '--build-manifest', str(args.build_manifest), '--target-mib', str(args.target_mib),
                   '--repeat', str(args.repeat)]
        receipt['sweep_command'] = command
        mc.dump(receipt_path, receipt)
        result = subprocess.run(command, env=env, timeout=850)
        receipt['sweep_exit_code'] = result.returncode
        result.check_returncode()
    finally:
        shutil.rmtree(decoder)
        receipt['decoder_removed'] = not decoder.exists()
        mc.dump(receipt_path, receipt)


if __name__ == '__main__':
    main()
