"""Require complete current-revision registry receipts and their actual logs.

This checks execution evidence, not the semantic adequacy of the underlying
checks. A cache hit, missing check, failed check or interrupted attempt is never
accepted as a successful verification profile.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from resource_group import require_limits

ROOT = Path(__file__).resolve().parents[2]
IDENTIFIER = re.compile(r'[a-z][a-z0-9-]*')


def selected_checks(registry: dict, profile: str) -> list[dict]:
    checks = registry.get('checks')
    if not isinstance(checks, list):
        raise ValueError('registry checks must be a list')
    selected = []
    seen = set()
    for check in checks:
        if not isinstance(check, dict):
            raise ValueError('invalid registry entry')
        identifier = check.get('id')
        if (not isinstance(identifier, str) or not IDENTIFIER.fullmatch(identifier)
                or identifier in seen):
            raise ValueError('invalid or duplicate check ID')
        seen.add(identifier)
        command = check.get('command')
        if (not isinstance(command, list) or not command
                or any(not isinstance(arg, str) for arg in command)):
            raise ValueError('invalid check command: ' + identifier)
        if check.get('profile') == profile:
            selected.append(check)
    if not selected:
        raise ValueError('profile has no checks: ' + profile)
    return selected


def sha256_file(path: Path) -> str:
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def verify(registry: dict, profile: str, commit: str, directory: Path) -> dict:
    if not re.fullmatch(r'[0-9a-f]{40}', commit):
        raise ValueError('a complete checked-out commit identity is required')
    if (directory / 'current-check.json').exists():
        raise ValueError('an interrupted or still-running check remains')
    results = []
    for check in selected_checks(registry, profile):
        identifier = check['id']
        path = directory / (identifier + '.json')
        log = directory / (identifier + '.log')
        if path.is_symlink() or log.is_symlink():
            raise ValueError('receipt and log must not be symlinks: ' + identifier)
        receipt = json.loads(path.read_text())
        if not isinstance(receipt, dict):
            raise ValueError('invalid receipt: ' + identifier)
        expected = {
            'receipt_version': 1,
            'check_id': identifier,
            'candidate_commit': commit,
            'worktree_dirty': False,
            'profile': profile,
            'command': check['command'],
            'result': 'passed',
            'exit_code': 0,
            'output_log': identifier + '.log',
        }
        if any(receipt.get(key) != value for key, value in expected.items()):
            raise ValueError('failed, stale or mismatched receipt: ' + identifier)
        if type(receipt.get('exit_code')) is not int:
            raise ValueError('invalid exit status: ' + identifier)
        elapsed = receipt.get('duration_ms')
        if type(elapsed) is not int or elapsed < 0:
            raise ValueError('invalid duration: ' + identifier)
        if sha256_file(log) != receipt.get('output_sha256'):
            raise ValueError('missing or modified output evidence: ' + identifier)
        results.append({'check_id': identifier, 'duration_ms': elapsed})
    return {
        'candidate_commit': commit,
        'profile': profile,
        'status': 'passed',
        'check_count': len(results),
        'total_check_ms': sum(item['duration_ms'] for item in results),
        'checks_by_duration': sorted(results, key=lambda item: item['duration_ms'], reverse=True),
        'limits': 'Receipt integrity and completeness, not a security certification.',
    }


def main() -> int:
    require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', choices=('fast', 'extended'), required=True)
    parser.add_argument('--receipts', type=Path, default=ROOT / 'target/verification/receipts')
    args = parser.parse_args()
    try:
        commit = subprocess.check_output(
            ['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True, timeout=10).strip()
        registry = json.loads((ROOT / 'xtask/checks.json').read_text())
        result = verify(registry, args.profile, commit, args.receipts)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print('CI receipt verification FAILED: ' + str(error), file=sys.stderr)
        return 1
    print(json.dumps(result, indent=2), flush=True)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
