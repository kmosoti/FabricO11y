"""Reconcile explicit capacity assignments without relaxing existing caps."""
import json
from pathlib import Path

import campaign_summary as inventory


def owner(path):
    top = path.parts[0]
    archive = 'lab-completion-run-01/memory/failure-catalog-many-s-64-r-1-plain-p-1.tar'
    if (top.startswith(inventory.CAPACITY_EXCLUSIVE_PREFIXES + ('catalog-baseline-freeze-',))
            or str(path) in (archive + '.gz', archive + '.manifest.json')
            or (path.parent == Path('lab-completion-run-01/memory')
                and path.name.startswith('failure-catalog-coupled-'))):
        return 'capacity'
    if top.startswith(inventory.QUERY_EXCLUSIVE_PREFIXES):
        return 'query'
    return 'shared'


def query_charge(records, base):
    owners = {}
    for path, row in records.items():
        owners.setdefault(tuple(row['inode']), set()).add(owner(Path(path).relative_to(base)))
    return {path: row for path, row in records.items()
            if owners[tuple(row['inode'])] != {'capacity'}}


def controls():
    import coupled_admit
    base = Path('/evidence')
    row = {'inode': (1, 2), 'bytes': 8, 'allocated': 4096}
    capacity = str(base / 'catalog-baseline-freeze-01/bin.gz')
    shared = str(base / 'lab-completion-run-01/coordinator/job/source.tar.gz')
    if query_charge({capacity: row}, base):
        raise RuntimeError('explicit capacity assignment still charged to query')
    if len(query_charge({capacity: row, shared: row}, base)) != 2:
        raise RuntimeError('shared inode alias escaped query charge')
    if owner(Path('lab-completion-run-01/memory/unassigned.bin')) != 'shared':
        raise RuntimeError('unassigned memory path excluded without evidence')
    if owner(Path('unassigned/catalog-baseline-freeze-01/bin.gz')) != 'shared':
        raise RuntimeError('nested lookalike excluded without assignment')
    for key in ('unique_inode_bytes', 'allocated_block_bytes'):
        usage = {'unique_inode_bytes': 0, 'allocated_block_bytes': 0, key: 1024**3 + 1}
        try:
            coupled_admit.fits(usage, 0, 1024**3)
        except RuntimeError:
            continue
        raise RuntimeError('overage accepted')


def query_observation(report, base):
    roots = [base / root for category in report['categories'].values() for root in category['roots']]
    records = inventory.file_records(roots, base=base)
    charged = inventory.footprint(query_charge(records, base))
    charged['allocated_block_bytes'] += report['symlink_allocated_blocks_bytes_separate']
    return charged


def main():
    import coupled_admit
    inventory.require_limits()
    controls()
    base = inventory.ROOT / 'docs/experiments/benchmarks/data'
    report = inventory.evidence_inventory(base)
    charged = query_observation(report, base)
    coupled_admit.fits(charged, 0, 1024**3)
    admission = coupled_admit.observe(0, 0)
    print(json.dumps({'basis': 'coupled-launch-protocol explicit capacity assignments; all other overhead remains query-chargeable',
                      'controls': 'explicit assignment, shared alias, unknown path, length/block overages',
                      'prior_conservative_query': report['conservative_lab_upper_bounds']['query'],
                      'reconciled_query_upper_bound': charged,
                      'query_cap_bytes': 1024**3, 'admission': admission,
                      'limitations': ['Historical failures remain; no deletion, cap increase or scientific gate change.',
                                      'Capacity is the assigned subset, not the old all-overhead upper bound.',
                                      'This job and later receipts add evidence after the snapshot.']}, indent=2))


if __name__ == '__main__':
    main()
