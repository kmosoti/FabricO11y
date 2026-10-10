#!/usr/bin/env python3
"""Read-only validation of the specification's original-source identities.

Does not edit, fetch, checkout, build, scan, or execute repository code.
Exit 0: original identities/anchors and current reviewed files match.
Exit 2: baseline/manifest invalid or unavailable.
Exit 3: baseline valid, but current files need re-anchoring/review.
"""
from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys


def git(repo: Path, *args: str) -> bytes:
    completed = subprocess.run(['git', '-C', str(repo), *args], capture_output=True, timeout=20)
    if completed.returncode:
        raise RuntimeError(completed.stderr.decode(errors='replace').strip())
    return completed.stdout


def safe_path(value: str) -> PurePosixPath:
    path = PurePosixPath(value)
    if not value or path.is_absolute() or '..' in path.parts:
        raise ValueError('unsafe manifest path')
    return path


def blob_hash(data: bytes) -> str:
    return hashlib.sha1(b'blob ' + str(len(data)).encode() + b'\0' + data).hexdigest()


def verify(repo: Path, manifest: dict) -> tuple[int, dict]:
    base = manifest['base_commit']
    if not re.fullmatch(r'[0-9a-f]{40}', base):
        raise ValueError('expected complete SHA-1 Git commit identity')
    root = Path(git(repo, 'rev-parse', '--show-toplevel').decode().strip()).resolve()
    actual = git(root, 'rev-parse', '--verify', base + '^{commit}').decode().strip()
    if actual != base:
        raise ValueError('base identity did not resolve exactly')
    head = git(root, 'rev-parse', 'HEAD').decode().strip()
    source_lines: dict[str, list[str]] = {}
    sources = []
    drift = []
    for item in manifest['locked_sources']:
        rel = safe_path(item['path'])
        data = git(root, 'cat-file', '-p', base + ':' + rel.as_posix())
        observed = blob_hash(data)
        if observed != item['blob_sha']:
            raise ValueError('original blob mismatch: ' + rel.as_posix())
        source_lines[rel.as_posix()] = data.decode('utf-8').splitlines()
        current = root.joinpath(*rel.parts)
        if (current.is_symlink() or not current.is_file()
                or not current.resolve().is_relative_to(root)):
            matches = False
        else:
            matches = blob_hash(current.read_bytes()) == observed
        sources.append({'path': rel.as_posix(), 'base_blob_verified': True,
                        'working_file_matches': matches})
        if not matches:
            drift.append(rel.as_posix())
    spans = []
    for item in manifest['edits']:
        lines = source_lines[item['path']]
        start, end = item['original_start_line'], item['original_end_line']
        if not (1 <= start <= end <= len(lines)):
            raise ValueError('out-of-range original span: ' + item['id'])
        if item['symbol_or_text_anchor'] not in '\n'.join(lines[start-1:end]):
            raise ValueError('original anchor absent from declared span: ' + item['id'])
        spans.append({'edit': item['id'], 'original_span_and_anchor_verified': True})
    return (3 if drift else 0), {
        'base_commit': base, 'current_head': head, 'original_sources': sources,
        'original_edit_spans': spans, 'files_requiring_reanchor': drift,
        'status': 'reanchor_required' if drift else 'reviewed_sources_match',
        'repository_mutated': False, 'application_tests_run': False,
        'note': 'Matching sources do not establish correctness or validate unreviewed files.'}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('repository', type=Path)
    parser.add_argument('--manifest', type=Path,
                        default=Path(__file__).with_name('change-manifest.json'))
    args = parser.parse_args()
    try:
        status, result = verify(args.repository, json.loads(args.manifest.read_text()))
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.TimeoutExpired) as exc:
        status, result = 2, {'status': 'baseline_unavailable_or_invalid', 'error': str(exc),
                             'repository_mutated': False, 'application_tests_run': False}
    print(json.dumps(result, indent=2))
    return status

if __name__ == '__main__':
    raise SystemExit(main())
