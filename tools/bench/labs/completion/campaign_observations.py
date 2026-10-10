#!/usr/bin/env python3
"""Report actual paired measurements and separately scoped scratch-byte receipts."""
import argparse
import json
from pathlib import Path
import statistics
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import STORAGE, require_limits
import builder


def read(path):
    return json.loads(path.read_text())


def main():
    require_limits()
    parser = argparse.ArgumentParser()
    parser.add_argument('--campaign-root', type=Path, required=True)
    parser.add_argument('--row-spill-root', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    if args.out.exists() or not args.out.parent.resolve().is_relative_to(STORAGE.resolve()):
        raise RuntimeError('fresh external data-drive result required')
    pages = read(args.campaign_root / 'page-blob-summary.json')
    cells = {}
    for shape, mib in builder.CELLS:
        name = f'{shape}-{mib}'
        cell = args.campaign_root / ('builder-' + name)
        pairs = [read(cell / f'pair-{i}.json') for i in range(1, 4)]
        spill_path = args.row_spill_root / (name + '.json')
        spill = read(spill_path)
        arms = {}
        for arm in ('reference', 'bounded'):
            rows = [pair[arm] for pair in pairs]
            arms[arm] = {
                'build_seconds': [row['build_seconds'] for row in rows],
                'median_build_seconds': statistics.median(row['build_seconds'] for row in rows),
                'build_cpu_seconds': [row['build_cpu_user_s'] + row['build_cpu_system_s'] for row in rows],
                'median_cpu_seconds': statistics.median(row['build_cpu_user_s'] + row['build_cpu_system_s'] for row in rows),
                'incremental_peak_heap_bytes': [row['incremental_peak_heap_bytes'] for row in rows],
                'worst_incremental_peak_heap_bytes': max(row['incremental_peak_heap_bytes'] for row in rows),
                'io_delta': [row['io_delta'] for row in rows],
                'median_io_delta': {key: statistics.median(row['io_delta'][key] for row in rows)
                                    for key in rows[0]['io_delta']},
            }
        page_bytes = pages['cells'][name][0]['logical_page_blob_bytes']
        cells[name] = {'arms': arms, 'row_run_spill_payload_bytes': spill['spill']['logical_bytes'],
                       'row_run_successful_write_syscalls': spill['spill']['successful_syscalls'],
                       'encoded_page_spill_blob_bytes': page_bytes,
                       'combined_logical_scratch_writes': spill['spill']['logical_bytes'] + page_bytes,
                       'row_spill_receipt': str(spill_path), 'row_spill_receipt_sha256': builder.sha(spill_path),
                       'same_binary_manifest': spill['manifest_identical'],
                       'campaign_result': read(cell / 'result.json')}
    builder.write(args.out, {'command': sys.argv, 'reporter_sha256': builder.sha(Path(__file__)),
                            'freeze': read(args.campaign_root / 'freeze.json'), 'cells': cells,
                            'scope': 'timing/CPU/heap are three paired builds; row spill is one supplemental replay; page blobs from paired stderr',
                            'combined_byte_scope': 'row-run file payload writes plus completed encoded page blob writes; excludes filesystem metadata and physical-device traffic',
                            'full_bs_acceptance': False})
    print(json.dumps({name: {key: row[key] for key in ('row_run_spill_payload_bytes', 'encoded_page_spill_blob_bytes', 'combined_logical_scratch_writes')}
                      for name, row in cells.items()}))


if __name__ == '__main__':
    main()
