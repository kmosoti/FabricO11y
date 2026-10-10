#!/usr/bin/env python3
"""Summarize counted page blobs separately from the existing row-run observer."""
import argparse
import copy
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import STORAGE, require_limits
import builder


def tables_from(text):
    tables = {}
    for line in text.splitlines():
        if not line.startswith('fabric-page-store: '):
            continue
        row = json.loads(line.removeprefix('fabric-page-store: '))
        name = row['table']
        if isinstance(name, dict):
            name = bytes(name['Unix']).decode('utf-8')
        if name in tables:
            raise RuntimeError('duplicate table observation')
        if not row['cleanup'] or row['outstanding_blob_bytes'] or row['puts'] != row['takes']:
            raise RuntimeError('incomplete blob lifecycle')
        tables[name] = row
    if set(tables) != {'batches', 'gaps', 'logs', 'metrics'}:
        raise RuntimeError('missing or unexpected fixture table observations')
    return tables


def sensitivity_controls(text):
    actual = tables_from(text)
    controls = {'unchanged_positive': {'accepted': True}}
    for defect in ('malformed', 'unbalanced', 'cleanup_false'):
        altered = copy.deepcopy(actual)
        altered['logs']['puts'] += defect == 'unbalanced'
        if defect == 'cleanup_false':
            altered['logs']['cleanup'] = False
        lines = ['fabric-page-store: ' + json.dumps(row) for row in altered.values()]
        if defect == 'malformed':
            lines[0] = 'fabric-page-store: {broken'
        mutated = '\n'.join(lines)
        try:
            tables_from(mutated)
        except (ValueError, RuntimeError, KeyError, TypeError) as error:
            controls[defect] = {'rejected': True, 'exception': str(error), 'injected_observations': lines}
        else:
            raise RuntimeError('page observation control was not rejected: ' + defect)
    return controls


def main():
    require_limits()
    parser = argparse.ArgumentParser()
    parser.add_argument('--campaign-root', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    control_out = args.out.with_suffix('.controls.json')
    if args.out.exists() or control_out.exists() or not args.out.parent.resolve().is_relative_to(STORAGE.resolve()):
        raise RuntimeError('fresh external data-drive result required')
    control_source = args.campaign_root / 'builder-steady-64/1-bounded.stderr.txt'
    builder.write(control_out, sensitivity_controls(control_source.read_text()))
    cells = {}
    for shape, mib in builder.CELLS:
        cell = f'{shape}-{mib}'
        observations = []
        for repetition in range(1, 4):
            source = args.campaign_root / ('builder-' + cell) / f'{repetition}-bounded.stderr.txt'
            tables = tables_from(source.read_text())
            observations.append({'repetition': repetition, 'source': str(source),
                                 'source_sha256': builder.sha(source), 'tables': tables,
                                 'logical_page_blob_bytes': sum(row['completed_blob_bytes'] for row in tables.values())})
        if len({row['logical_page_blob_bytes'] for row in observations}) != 1:
            raise RuntimeError('page byte totals changed across fixed-input repetitions')
        cells[cell] = observations
    builder.write(args.out, {'command': sys.argv, 'campaign_root': str(args.campaign_root),
                            'checker_sha256': builder.sha(Path(__file__)),
                            'metric': 'completed serialized page blob bytes written to scratch',
                            'excludes': 'row-run spill, scratch metadata, physical device traffic',
                            'cells': cells})
    print(json.dumps({cell: rows[0]['logical_page_blob_bytes'] for cell, rows in cells.items()}))


if __name__ == '__main__':
    main()
