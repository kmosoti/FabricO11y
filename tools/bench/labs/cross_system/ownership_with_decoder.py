"""Ephemeral isolated validation provider around one ownership cell job."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

import memory_census as mc
from ownership_sweep import REGISTERED


def main():
    mc.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--destination', type=Path, required=True)
    parser.add_argument('--protocol', type=Path, required=True)
    parser.add_argument('--build-manifest', type=Path, required=True)
    parser.add_argument('--stage', choices=['screen', 'confirmation'], required=True)
    parser.add_argument('--repeat', type=int, choices=[0, 1], required=True)
    parser.add_argument('--screen-receipts', type=Path, nargs=2)
    args = parser.parse_args()
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    destination = args.destination.resolve()
    receipt_path = destination.parent/(destination.name+'-decoder.json')
    if not scratch.is_relative_to(mc.DATA.resolve()) or not destination.is_relative_to(REGISTERED.resolve()) or destination.exists() or receipt_path.exists():
        raise ValueError('fresh registered evidence and owned data-drive scratch required')
    destination.parent.mkdir(parents=True, exist_ok=True)
    decoder = Path(tempfile.mkdtemp(prefix='ownership-decoder-', dir=scratch))
    env = dict(os.environ, UV_PYTHON_DOWNLOADS='never', PYTHONDONTWRITEBYTECODE='1')
    command = ['uv', 'pip', 'install', '--no-cache', '--only-binary', ':all:',
               '--python', sys.executable, '--target', str(decoder), 'pyarrow==22.0.0']
    receipt = {'install_command': command, 'decoder': str(decoder), 'role': 'replaceable validation dependency'}
    try:
        result = subprocess.run(command, env=env, timeout=45)
        receipt['install_exit_code'] = result.returncode
        result.check_returncode()
        receipt['package_metadata_sha256'] = {str(p.relative_to(decoder)): mc.digest(p)
                for p in decoder.glob('*.dist-info/*') if p.is_file()}
        env['PYTHONPATH'] = str(decoder)
        command = [sys.executable, '-B', str(Path(__file__).with_name('ownership_sweep.py')), 'run',
            '--destination', str(destination), '--protocol', str(args.protocol),
            '--build-manifest', str(args.build_manifest), '--stage', args.stage, '--repeat', str(args.repeat)]
        if args.screen_receipts:
            command += ['--screen-receipts', *(str(p) for p in args.screen_receipts)]
        receipt['worker_command'] = command
        mc.dump(receipt_path, receipt)
        result = subprocess.run(command, env=env, timeout=850)
        receipt['worker_exit_code'] = result.returncode
        result.check_returncode()
    finally:
        shutil.rmtree(decoder)
        receipt['decoder_removed'] = not decoder.exists()
        mc.dump(receipt_path, receipt)


if __name__ == '__main__':
    main()
