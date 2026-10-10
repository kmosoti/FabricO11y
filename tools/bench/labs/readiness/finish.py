#!/usr/bin/env python3
"""Final sequential validation and compact receipts for this investigation."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

ROOT = Path(__file__).resolve().parents[4]
DATA = ROOT / 'docs/experiments/benchmarks/data/readiness-labs-run-01'


def main():
    out = DATA / 'verification'
    out.mkdir(exist_ok=False)
    commands = [
        ('accounting-reviewed', ['python3', '-B', 'tools/bench/labs/readiness/query/audit.py',
             '--cells', 'off', 'scan', '--out', str(DATA / 'query/audit-reviewed.json')]),
        ('cleanup', ['python3', '-B', 'tools/bench/labs/readiness/collect.py']),
        ('fast', ['cargo', 'xtask', 'checks', '--profile', 'fast']),
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
        if name == 'fast':
            receipts = out / 'fast-receipts'
            receipts.mkdir()
            policy = json.loads((ROOT / 'xtask/checks.json').read_text())
            for check in policy['checks']:
                if check['profile'] == 'fast':
                    source = ROOT / 'target/verification/receipts' / (check['id'] + '.json')
                    if source.exists():
                        shutil.copyfile(source, receipts / source.name)
        (out / 'commands.json').write_text(json.dumps(records, indent=2) + '\n')
        print(json.dumps(records[-1]), flush=True)
        if child.returncode:
            raise SystemExit(child.returncode)


if __name__ == '__main__':
    main()
