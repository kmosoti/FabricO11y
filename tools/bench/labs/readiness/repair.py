#!/usr/bin/env python3
"""Complete the failed lint check without repeating successful workload tests."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

ROOT = Path(__file__).resolve().parents[4]
DATA = ROOT / 'docs/experiments/benchmarks/data/readiness-labs-run-01'


def main():
    out = DATA / 'verification-repair'
    out.mkdir(exist_ok=False)
    commands = [
        ('fmt', ['cargo', 'xtask', 'checks', '--profile', 'fast', '--only', 'fmt', '--receipts', str(out / 'receipts')]),
        ('clippy', ['cargo', 'xtask', 'checks', '--profile', 'fast', '--only', 'clippy', '--receipts', str(out / 'receipts')]),
        ('resource-launcher', ['python3', '-B', 'tools/test_resource_group.py']),
        ('documentation', ['bun', 'tools/docs/check.mjs']),
    ]
    records = []
    for name, command in commands:
        started = time.monotonic()
        with (out / (name + '.txt')).open('w') as stream:
            child = subprocess.run(command, cwd=ROOT, stdout=stream, stderr=subprocess.STDOUT,
                                   env=dict(os.environ, CARGO_NET_OFFLINE='true'))
        records.append({'name': name, 'command': command, 'exit': child.returncode,
                        'elapsed_seconds': time.monotonic() - started})
        (out / 'commands.json').write_text(json.dumps(records, indent=2) + '\n')
        print(json.dumps(records[-1]), flush=True)
        if child.returncode:
            raise SystemExit(child.returncode)
    first = json.loads((DATA / 'coordinator/v1-checks/receipt.json').read_text())
    unit = Path(first['cgroup']).name.removesuffix('.service')
    path = ROOT / 'target/resource-containment/runs' / (unit + '.json')
    launcher = json.loads(path.read_text())
    shutil.copyfile(path, DATA / 'launcher-receipts' / path.name)
    owned = Path('/run/media/kmosoti/data/FabricO11y/evidence') / unit
    if launcher['retained_failure_evidence'] != str(owned) or owned.is_symlink() or first['exit'] != 1:
        raise RuntimeError('verification scratch ownership differs')
    size = sum(p.stat().st_size for p in owned.rglob('*') if p.is_file())
    shutil.rmtree(owned)
    (out / 'cleanup.json').write_text(json.dumps({'owned_path': str(owned),
        'logical_bytes_removed': size, 'removed': not owned.exists(),
        'original_failed_receipt_unchanged': True}, indent=2) + '\n')
    sizes = {lab: sum(p.stat().st_size for p in (DATA / lab).rglob('*') if p.is_file())
             for lab in ('memory', 'query', 'recovery', 'coordinator')}
    (out / 'retained-evidence-bytes.json').write_text(json.dumps(sizes, indent=2) + '\n')
    if any(n > 50 * 1024**2 for n in sizes.values()):
        raise RuntimeError('compact lab evidence exceeds its bound')


if __name__ == '__main__':
    main()
