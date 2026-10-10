"""Prospective coupled-lab resource admission; preserve historical overages."""
import argparse
import json
import subprocess
from pathlib import Path
import campaign_summary as inventory


def fits(usage, reserve, cap):
    if reserve < 0 or max(usage['unique_inode_bytes'], usage['allocated_block_bytes']) + reserve > cap:
        raise RuntimeError('projected evidence exceeds allocation')


def observe(reserve, capacity_reserve):
    from coupled_resource_readiness import query_observation
    base = inventory.ROOT / 'docs/experiments/benchmarks/data'
    report = inventory.evidence_inventory(base)
    total = dict(report['catalog_complete_owned_persistent'])
    total['allocated_block_bytes'] = report['aggregate_allocated_bytes_including_object_links']
    capacity = dict(report['capacity_mandatory_lower_bound'])
    extra = inventory.file_records(list((base / 'lab-completion-run-01/memory').glob('failure-catalog-coupled-*')))
    # New capacity failures are distinct from the mandatory historical CR3 archive.
    more = inventory.footprint(extra)
    for key in ('unique_inode_bytes', 'allocated_block_bytes'):
        capacity[key] += more[key]
    fits(total, reserve, min(2 * 1024**3, 1_693_552_640 + 192 * 1024**2))
    fits(capacity, capacity_reserve, 832 * 1024**2)
    query = query_observation(report, base)
    fits(query, reserve, 1024**3)
    return {'aggregate': total, 'capacity': capacity,
            'query_conservative_upper_bound': query,
            'original_capacity_status': report['capacity_mandatory_lower_bound']['status'],
            'prospective_capacity_cap_bytes': 832 * 1024**2,
            'reserve_bytes': reserve, 'capacity_reserve_bytes': capacity_reserve}


def main():
    inventory.require_limits()
    for key in ('unique_inode_bytes', 'allocated_block_bytes'):
        for cap in (2 * 1024**3, 832 * 1024**2):
            defect = {'unique_inode_bytes': 0, 'allocated_block_bytes': 0, key: cap + 1}
            try:
                fits(defect, 0, cap)
            except RuntimeError:
                pass
            else:
                raise RuntimeError('overage control accepted')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--reserve-mib', type=int, required=True)
    parser.add_argument('--capacity-reserve-mib', type=int, default=0)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if not 0 <= args.capacity_reserve_mib <= args.reserve_mib <= 192:
        parser.error('reserve outside registered incremental envelope')
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command:
        parser.error('command required')
    before = observe(args.reserve_mib * 1024**2, args.capacity_reserve_mib * 1024**2)
    print(json.dumps({'admission_before': before, 'overage_controls_rejected': True}), flush=True)
    status = subprocess.call(command, cwd=inventory.ROOT)
    after = observe(0, 0)
    print(json.dumps({'admission_after': after, 'child_exit': status}), flush=True)
    raise SystemExit(status)


if __name__ == '__main__':
    main()
