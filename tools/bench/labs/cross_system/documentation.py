"""Run manual documentation checks with the existing authenticated Bun artifact."""
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
    require_limits()
    base = ROOT / 'docs/experiments/benchmarks/data/cross-system-run-01'
    previous = base / 'coordinator/final-docs-01'
    for name in ('docs', 'docs-checker-probes', 'hooks'):
        source = ROOT / 'target/verification/receipts' / (name + '.json')
        target = previous / source.name
        if not target.exists():
            shutil.copyfile(source, target)
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
    receipt = {'archive': str(archive), 'archive_sha256': bun.ARCHIVE_SHA,
               'member': bun.MEMBER, 'payload_sha256': bun.PAYLOAD_SHA,
               'command': ['cargo', 'xtask', 'checks', '--profile', 'documentation']}
    if '--links-only' in sys.argv:
        receipt['command'] = ['bun', 'tools/docs/check.mjs']
    try:
        result = subprocess.run(receipt['command'], cwd=ROOT,
                                env=dict(os.environ, PATH=str(target.parent) + ':' + os.environ['PATH']))
        receipt['exit'] = result.returncode
        result.check_returncode()
    finally:
        bun.compare(archive, bun.MEMBER, target, bun.PAYLOAD_SHA, bun.PAYLOAD_BYTES)
        target.unlink()
        receipt['runtime_copy_removed'] = not target.exists()
        name = 'documentation-links-runtime.json' if '--links-only' in sys.argv else 'documentation-runtime.json'
        (base / name).write_text(json.dumps(receipt, indent=2) + '\n')


if __name__ == '__main__':
    main()
