"""Manual documentation verification using the already authenticated Bun artifact."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
sys.path.insert(0, str(ROOT / 'tools/bench/labs/catalog'))
import coupled_bun_preserve as bun
from resource_group import require_limits


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', required=True, type=Path)
    args = parser.parse_args()
    require_limits()
    args.out.mkdir(parents=True, exist_ok=False)
    receipts = ROOT / 'target/verification/receipts'
    previous = args.out / 'previous'
    previous.mkdir()
    names = ('docs', 'docs-checker-probes', 'hooks')
    for name in names:
        source = receipts / (name + '.json')
        if source.exists():
            shutil.copyfile(source, previous / source.name)
    archive = bun.BASE / bun.ARCHIVE
    if bun.sha(archive) != bun.ARCHIVE_SHA:
        raise RuntimeError('canonical runtime archive differs')
    target = Path(os.environ['FABRIC_SCRATCH_ROOT']) / 'bun'
    with tarfile.open(archive, 'r:gz') as stream:
        members = [m for m in stream.getmembers() if m.name == bun.MEMBER]
        if len(members) != 1 or not members[0].isfile() or members[0].size != bun.PAYLOAD_BYTES:
            raise RuntimeError('runtime member identity differs')
        with stream.extractfile(members[0]) as source, target.open('xb') as output:
            shutil.copyfileobj(source, output, 1024**2)
    bun.compare(archive, bun.MEMBER, target, bun.PAYLOAD_SHA, bun.PAYLOAD_BYTES)
    target.chmod(0o700)
    fresh_receipts = args.out.resolve() / 'checks'
    receipt = {'archive': str(archive), 'archive_sha256': bun.ARCHIVE_SHA,
               'member': bun.MEMBER, 'payload_sha256': bun.PAYLOAD_SHA,
               'command': ['cargo', 'xtask', 'checks', '--profile', 'documentation',
                           '--receipts', str(fresh_receipts)],
               'hooks_or_ci_enabled': False}
    try:
        result = subprocess.run(receipt['command'], cwd=ROOT,
                                env=dict(os.environ, CARGO_BUILD_JOBS='2',
                                         PATH=str(target.parent) + ':' + os.environ['PATH']))
        receipt['exit'] = result.returncode
        result.check_returncode()
    finally:
        for name in names:
            source = fresh_receipts / (name + '.json')
            if source.exists():
                shutil.copyfile(source, args.out / source.name)
        bun.compare(archive, bun.MEMBER, target, bun.PAYLOAD_SHA, bun.PAYLOAD_BYTES)
        target.unlink()
        receipt['runtime_copy_removed'] = not target.exists()
        (args.out / 'runtime.json').write_text(json.dumps(receipt, indent=2) + '\n')


if __name__ == '__main__':
    main()
