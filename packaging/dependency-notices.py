#!/usr/bin/env python3
"""Inventory license texts for the product's locked Linux x86_64 build graph."""
import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

PACKAGES = ('fabric', 'fabric-cli', 'fabric-query', 'fabric-server', 'fabric-mcp')
TARGET = 'x86_64-unknown-linux-gnu'
PREFIXES = ('LICENSE', 'LICENCE', 'COPYING', 'NOTICE', 'COPYRIGHT')


def collect(metadata, tree, destination):
    selected = set()
    for line in tree.splitlines():
        match = re.match(r'^([^ ]+) v([^ ]+)(?: |$)', line)
        if match:
            selected.add(match.groups())
    workspace = set(metadata['workspace_members'])
    packages = [p for p in metadata['packages'] if p['id'] not in workspace and
                (p['name'], p['version']) in selected]
    found = {(p['name'], p['version']) for p in metadata['packages']}
    if selected - found:
        raise ValueError('tree packages missing from metadata: ' + repr(sorted(selected - found)))
    entries, missing = [], []
    supplemental_root = Path(__file__).resolve().parent / 'licenses'
    supplemental = json.loads((supplemental_root / 'manifest.json').read_text())
    for p in sorted(packages, key=lambda p: (p['name'], p['version'], p['id'])):
        directory = Path(p['manifest_path']).parent
        provenance = None
        files = {f for f in directory.iterdir() if f.is_file() and not f.is_symlink()
                 and f.name.upper().startswith(PREFIXES)}
        if p.get('license_file'):
            f = directory / p['license_file']
            if f.is_file() and not f.is_symlink():
                files.add(f)
        if not files:
            records = [r for r in supplemental if r['name'] == p['name'] and r['version'] == p['version']]
            if len(records) == 1:
                record = records[0]
                vcs = json.loads((directory / '.cargo_vcs_info.json').read_text())
                f = supplemental_root / (p['name'] + '-' + p['version']) / 'LICENSE'
                if vcs['git']['sha1'] != record['commit'] or f.is_symlink() or hashlib.sha256(f.read_bytes()).hexdigest() != record['sha256']:
                    raise ValueError('invalid supplemental license provenance: ' + p['name'])
                files.add(f)
                provenance = record
        if not files:
            missing.append(f"{p['name']} {p['version']} ({p.get('license') or 'no SPDX expression'})")
        entries.append((p, files, provenance))
    if missing:
        raise ValueError('dependency license texts missing; supply verified upstream texts before distribution: ' + ', '.join(missing))
    destination.mkdir(parents=True, exist_ok=False)
    records = []
    for p, files, provenance in entries:
        target = destination / (p['name'] + '-' + p['version'])
        target.mkdir()
        for f in sorted(files):
            shutil.copyfile(f, target / f.name)
        records.append({'name': p['name'], 'version': p['version'], 'source': p['source'],
                        'license': p.get('license'), 'files': sorted(f.name for f in files), 'supplemental_provenance': provenance})
    (destination / 'inventory.json').write_text(json.dumps({'scope': 'locked Linux x86_64 product normal and build dependencies', 'packages': records}, indent=2) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('destination', type=Path)
    args = parser.parse_args()
    metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--offline', '--format-version', '1']))
    command = ['cargo', 'tree', '--locked', '--offline', '--target', TARGET, '-e', 'normal,build', '--prefix', 'none', '--format', '{p}']
    for package in PACKAGES:
        command += ['-p', package]
    tree = subprocess.check_output(command, text=True)
    collect(metadata, tree, args.destination)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as e:
        print(f'dependency notices: {e}', file=sys.stderr)
        sys.exit(1)
