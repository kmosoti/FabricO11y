"""Run focused security regressions with full, revision-bound diagnostic logs.

This supplements, never substitutes for, the existing fast/extended registry.
Formatting suggestions are generated on owned copies, not the tested checkout.
"""
from __future__ import annotations

import difflib
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from resource_group import STORAGE, require_limits

ROOT = Path(__file__).resolve().parents[2]
FORMAT_SOURCES = (
    'crates/fabric-server/src/peer_admission.rs',
    'crates/fabric-adapter-linux/src/log_source/secure_open.rs',
    'src/spindle/otlp.rs',
    'src/spindle/otlp/deadline_io.rs',
    'src/spindle/otlp/security_tests.rs',
)


def suggestions(out: Path) -> None:
    # Only deterministic formatter output on source copies. These bytes have
    # not been tested and are never written back or pushed automatically.
    with tempfile.TemporaryDirectory(
            prefix='security-format-', dir=os.environ['FABRIC_SCRATCH_ROOT']) as temporary:
        work = Path(temporary)
        for name in FORMAT_SOURCES:
            target = work / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, target)
        command = ['rustfmt', '--edition', '2024', *[str(work / name) for name in FORMAT_SOURCES]]
        with (out / 'format-suggestions.log').open('w') as log:
            process = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, timeout=120)
        if process.returncode != 0:
            raise RuntimeError('cannot produce formatter-only suggestions')
        patch = []
        for name in FORMAT_SOURCES:
            original = (ROOT / name).read_text()
            formatted = (work / name).read_text()
            if original != formatted:
                target = out / 'format-suggestions' / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(formatted)
                patch.extend(difflib.unified_diff(
                    original.splitlines(keepends=True), formatted.splitlines(keepends=True),
                    fromfile='a/' + name, tofile='b/' + name))
        (out / 'format-suggestions.patch').write_text(''.join(patch))


def main() -> int:
    group = require_limits()
    out = STORAGE / 'results/security-checks'
    out.mkdir(parents=True, exist_ok=True)
    commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    receipt = {'commit': commit, 'cgroup': str(group), 'results': [],
               'scope': 'focused regression tests, not complete security qualification',
               'status': 'started'}
    commands = [
        ('fmt', ['cargo', 'fmt', '--all', '--check'], 0),
        ('linux', ['cargo', 'test', '--locked', '-p', 'fabric-adapter-linux', '--lib', 'security_'], 7),
        ('node', ['cargo', 'test', '--locked', '-p', 'fabric_o11y', '--lib', 'security_'], 11),
        ('server', ['cargo', 'test', '--locked', '-p', 'fabric-server', '--lib', 'security_'], 7),
    ]
    passed = True
    for name, command, minimum_tests in commands:
        path = out / (name + '.log')
        try:
            with path.open('w') as log:
                process = subprocess.run(command, cwd=ROOT, stdout=log,
                                         stderr=subprocess.STDOUT, timeout=1200)
            text = path.read_text()
            counts = re.findall(r'test result: ok\. (\d+) passed; 0 failed;', text)
            count = sum(int(value) for value in counts)
            accepted = process.returncode == 0 and count >= minimum_tests
            result = {'name': name, 'command': command, 'exit': process.returncode,
                      'status': 'passed' if accepted else 'failed',
                      'tests_passed': count, 'minimum_tests': minimum_tests,
                      'log_sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
        except (OSError, subprocess.TimeoutExpired) as error:
            accepted = False
            result = {'name': name, 'command': command, 'exit': None,
                      'status': 'incomplete', 'error': str(error)}
        passed = passed and accepted
        receipt['results'].append(result)
        print(json.dumps(result), flush=True)
        if not accepted and path.exists():
            print(path.read_text()[-24000:], flush=True)
        (out / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    try:
        suggestions(out)
    except (OSError, RuntimeError, KeyError, subprocess.TimeoutExpired) as error:
        receipt['suggestions_error'] = str(error)
        passed = False
    receipt['status'] = 'passed' if passed else 'failed'
    receipt['exit'] = 0 if passed else 1
    (out / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    return receipt['exit']


if __name__ == '__main__':
    raise SystemExit(main())
