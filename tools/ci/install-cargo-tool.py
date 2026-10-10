"""Install one pinned CI tool with a temporary target and cleanup receipt."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from resource_group import require_limits, STORAGE


def main():
    require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('crate')
    parser.add_argument('version')
    args = parser.parse_args()
    if not args.crate.replace('-', '').isalnum() or not all(c.isdigit() or c == '.' for c in args.version):
        raise ValueError('expected explicit crate name and numeric pinned version')
    if shutil.disk_usage(STORAGE).free < 2 * 1024**3:
        raise RuntimeError('CI tool install requires 2 GiB available disk headroom')
    scratch = Path(tempfile.mkdtemp(prefix='cargo-tool-', dir=os.environ['FABRIC_SCRATCH_ROOT']))
    receipt = {'crate': args.crate, 'version': args.version, 'target': str(scratch), 'exit': None}
    out = STORAGE / 'results/ci-tool-install'
    out.mkdir(parents=True, exist_ok=True)
    try:
        command = ['cargo', 'install', args.crate, '--version', args.version, '--locked', '--target-dir', str(scratch)]
        receipt['command'] = command
        with (out / (args.crate + '.log')).open('w') as log:
            receipt['exit'] = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, timeout=1500).returncode
        if receipt['exit']:
            raise RuntimeError('pinned Cargo tool install failed; retained log: ' + str(out))
    except BaseException as error:
        receipt['error'] = str(error)
        raise
    finally:
        shutil.rmtree(scratch)
        receipt['cleanup_absent'] = not scratch.exists()
        receipt['disk_after'] = dict(zip(('total', 'used', 'free'), shutil.disk_usage(STORAGE)))
        (out / (args.crate + '.json')).write_text(json.dumps(receipt, indent=2) + '\n')
    return 0


if __name__ == '__main__':
    sys.exit(main())
