"""Inventory actual catalog job resources; completion is distinct from performance."""
import gzip
import json
import os
import stat
import tempfile
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits


def counters(raw):
    return {key: int(value) for key, value in (line.split() for line in raw.splitlines())}



def verify_limits(limits):
    if limits['memory.max'] != str(20 * 1024**3) or limits['memory.swap.max'] != '0':
        raise RuntimeError('catalog containment differs')



def known_object_links(base):
    links = {}
    for pair in (2, 3):
        links[base / f'catalog-boundary-pair{pair}-01/objects'] = base / 'catalog-boundary-diagnostic-01/objects'
    for slice_name in ('freeze', 'plain-1', 'plain-2', 'plain-3', 'counted-1', 'counted-2', 'counted-3'):
        links[base / f'catalog-borrowed-log-run-01/{slice_name}/objects'] = base / 'catalog-borrowed-log-run-01/objects'
    for rate in (0, 1, 4):
        for slice_name in ('preflight-01', 'diagnostic-01', 'pair1-01', 'pair2-01', 'pair3-01'):
            links[base / f'catalog-maintenance-{rate}-{slice_name}/objects'] = base / 'catalog-maintenance-objects-01'
    links[base / 'catalog-maintenance-0-preflight-02/objects'] = base / 'catalog-maintenance-objects-01'
    for slice_name in ('preflight', 'pair1', 'diagnostic'):
        links[base / f'catalog-boundary-reuse-{slice_name}-01/objects'] = base / 'catalog-boundary-reuse-objects-01'
    return links


def file_records(roots, base=None, approved_links=None, aliases=None):
    records, found_links = {}, []
    for root in roots:
        paths = [root] if root.is_file() or root.is_symlink() else root.rglob('*')
        for path in paths:
            st = path.lstat()
            if stat.S_ISLNK(st.st_mode):
                found_links.append(path)
            elif stat.S_ISREG(st.st_mode):
                records[str(path)] = {'inode': (st.st_dev, st.st_ino),
                    'bytes': st.st_size, 'allocated': st.st_blocks * 512}
    present = {tuple(row['inode']) for row in records.values()}
    approved = approved_links if approved_links is not None else (known_object_links(base) if base is not None else {})
    for path in found_links:
        expected = approved.get(path)
        if base is None or path.name != 'objects' or expected is None:
            raise RuntimeError('unexpected linked evidence: ' + str(path))
        if expected.is_symlink() or not expected.is_dir():
            raise RuntimeError('object link target missing or indirect: ' + str(path))
        target = path.resolve(strict=True)
        if target != expected.resolve(strict=True) or not target.is_relative_to(base.resolve()):
            raise RuntimeError('object link target outside exact owned allowlist: ' + str(path))
        target_records = file_records([target])  # Reject target-internal links/cycles.
        if not {tuple(row['inode']) for row in target_records.values()} <= present:
            raise RuntimeError('object link target not independently inventoried: ' + str(path))
        if aliases is not None:
            aliases[str(path)] = {'target': str(target),
                'regular_logical_alias_bytes': sum(row['bytes'] for row in target_records.values()),
                'target_regular_files': len(target_records),
                'link_allocated_blocks_bytes': path.lstat().st_blocks * 512,
                'link_inode': (path.lstat().st_dev, path.lstat().st_ino)}
    return records

def footprint(records):
    unique = {tuple(row['inode']): row for row in records.values()}
    return {'files': len(records), 'unique_inodes': len(unique),
        'logical_path_bytes': sum(row['bytes'] for row in records.values()),
        'unique_inode_bytes': sum(row['bytes'] for row in unique.values()),
        'allocated_block_bytes': sum(row['allocated'] for row in unique.values())}


def enforce_catalog_budget(observation, maximum=2 * 1024**3):
    if max(observation['unique_inode_bytes'], observation['allocated_block_bytes']) > maximum:
        raise RuntimeError('complete persistent catalog evidence exceeds registered aggregate cap: ' + json.dumps(observation))



# Only top-level dataset families assigned by the registered allocation are
# exclusive. Baseline freezes, shared pools and every legacy/archive/receipt
# path remain uncertain and are deliberately charged to both labs.
CAPACITY_EXCLUSIVE_PREFIXES = ('catalog-spill-', 'catalog-builder-spill-',
    'catalog-borrowed-', 'catalog-many-segment-', 'catalog-lifetime-')
QUERY_EXCLUSIVE_PREFIXES = ('catalog-boundary-', 'catalog-maintenance-',
    'catalog-metadata-', 'catalog-preboundary-', 'catalog-evidence-pool-',
    'catalog-evidence-compaction-')


def capacity_mandatory_subset(records, base):
    archive = 'lab-completion-run-01/memory/failure-catalog-many-s-64-r-1-plain-p-1.tar'
    selected = {path: row for path, row in records.items()
                if Path(path).relative_to(base).parts[0].startswith(
                    CAPACITY_EXCLUSIVE_PREFIXES + ('catalog-baseline-freeze-',))
                or str(Path(path).relative_to(base)) in (archive + '.gz', archive + '.manifest.json')}
    observation = footprint(selected)
    cap = 768 * 1024**2
    excess = max(observation['unique_inode_bytes'], observation['allocated_block_bytes']) - cap
    return {**observation, 'registered_cap_bytes': cap,
            'excess_bytes': max(0, excess),
            'status': 'confirmed allowance violation' if excess > 0 else 'subset within cap; not full certification',
            'scope': 'mandatory capacity datasets, baseline and CR3 failure archive; overhead excluded'}


def conservative_lab_bounds(records, base):
    owners = {}
    for path, row in records.items():
        relative = Path(path).relative_to(base)
        top = relative.parts[0]
        owner = 'both'
        if top.startswith(CAPACITY_EXCLUSIVE_PREFIXES):
            owner = 'capacity'
        elif top.startswith(QUERY_EXCLUSIVE_PREFIXES):
            owner = 'query'
        inode = tuple(row['inode'])
        owners.setdefault(inode, set()).add(owner)
    result = {}
    for lab, cap in [('capacity', 768 * 1024**2), ('query', 1024**3)]:
        # Any uncertain or cross-lab alias makes the whole inode chargeable to
        # both. Exclude only inodes whose EVERY observed path is other-exclusive.
        other = 'query' if lab == 'capacity' else 'capacity'
        charged = {path: row for path, row in records.items()
                   if owners[tuple(row['inode'])] != {other}}
        observation = footprint(charged)
        fits = max(observation['unique_inode_bytes'], observation['allocated_block_bytes']) <= cap
        result[lab] = {**observation, 'registered_cap_bytes': cap,
            'certified_by_conservative_upper_bound': fits,
            'status': 'certified within cap' if fits else 'inconclusive: refine attribution without changing cap'}
    return result


def evidence_inventory(base):
    legacy = base / 'lab-completion-run-01'
    categories = {'root_catalog_datasets_and_files': list(base.glob('catalog-*'))}
    for lab in ('coordinator', 'memory', 'query', 'recovery'):
        directory = legacy / lab
        categories[lab + '_catalog_job_directories_and_files'] = list(directory.glob('catalog-*'))
        categories[lab + '_failure_archives_and_manifests'] = list(directory.glob('failure-catalog-*'))
    categories['launcher_receipt_files'] = list((legacy / 'coordinator/launcher-receipts').glob('catalog-*'))
    categories['root_cleanup_receipt_files'] = list(legacy.glob('catalog*.json'))
    all_records, category_rows, aliases = {}, {}, {}
    for category, roots in categories.items():
        records = file_records(roots, base=base, aliases=aliases)
        all_records.update(records)
        category_rows[category] = {'roots': [str(root.relative_to(base)) for root in roots],
                                   **footprint(records)}
    total = footprint(all_records)
    link_blocks = sum({tuple(row['link_inode']): row['link_allocated_blocks_bytes']
                       for row in aliases.values()}.values())
    aggregate_with_links = dict(total, allocated_block_bytes=total['allocated_block_bytes'] + link_blocks)
    enforce_catalog_budget(aggregate_with_links)
    lab_bounds = conservative_lab_bounds(all_records, base)
    for bound in lab_bounds.values():
        bound['allocated_block_bytes'] += link_blocks  # Uncertain link storage charged to both.
        fits = max(bound['unique_inode_bytes'], bound['allocated_block_bytes']) <= bound['registered_cap_bytes']
        bound['certified_by_conservative_upper_bound'] = fits
        bound['status'] = 'certified within cap' if fits else 'inconclusive: refine attribution without changing cap'
    return {'catalog_complete_owned_persistent': total, 'categories': category_rows,
        'capacity_mandatory_lower_bound': capacity_mandatory_subset(all_records, base),
        'approved_object_directory_aliases': aliases,
        'directory_alias_logical_bytes_separate': sum(row['regular_logical_alias_bytes'] for row in aliases.values()),
        'symlink_allocated_blocks_bytes_separate': sum(row['link_allocated_blocks_bytes'] for row in aliases.values()),
        'aggregate_allocated_bytes_including_object_links': aggregate_with_links['allocated_block_bytes'],
        'conservative_lab_upper_bounds': lab_bounds,
        'broader_legacy_campaign_including_catalog': footprint(file_records([legacy], base=base)),
        'all_benchmark_data_including_catalog_and_legacy': footprint(file_records([base], base=base)),
        'registered_caps_bytes': {'catalog_aggregate': 2 * 1024**3,
                                 'capacity': 768 * 1024**2, 'query': 1024**3},
        'enforced_here': 'complete aggregate; original conservative bounds and mandatory capacity lower bound',
        'exclusive_top_level_prefixes': {'capacity': CAPACITY_EXCLUSIVE_PREFIXES,
                                         'query': QUERY_EXCLUSIVE_PREFIXES},
        'scope_limits': [
            'Categories describe locations; per-lab bounds exclude only other-exclusive top-level dataset inodes.',
            'Uncertain, baseline/shared, legacy overhead and cross-lab aliased inodes are charged to BOTH labs.',
            'Category physical totals overlap on shared inodes and must not be summed.',
            'Per-lab certification requires BOTH unique lengths and allocated blocks to fit the conservative bound.',
            'An upper-bound miss is inconclusive and requires attribution review; it does not relax a cap.',
            'Frozen drivers use narrower prefix inventories and miss failure/launcher/cleanup files.',
            'Broader legacy/all-data totals include catalog and are not additive to its total.',
            'Known object-directory aliases are not recursively expanded into logical_path_bytes; alias bytes/link blocks are separate.',
            'Owned retained failure scratch outside benchmark data is excluded until verified archive cleanup.',
            'The audit output and still-running summary receipt complete after this snapshot.']}


def controls():
    base = Path('/synthetic')
    sample = {}
    for index, name in enumerate(['catalog-baseline-freeze-01/file',
            'lab-completion-run-01/memory/failure-catalog-many-s-64-r-1-plain-p-1.tar.gz',
            'lab-completion-run-01/memory/failure-catalog-many-s-64-r-1-plain-p-1.tar.manifest.json',
            'lab-completion-run-01/coordinator/catalog-overhead/file']):
        sample[str(base / name)] = {'inode': (1, index), 'bytes': 3, 'allocated': 4}
    sample[str(base / 'catalog-spill-alias/file')] = dict(next(iter(sample.values())))
    subset = capacity_mandatory_subset(sample, base)
    if subset['unique_inode_bytes'] != 9 or subset['allocated_block_bytes'] != 12:
        raise RuntimeError('capacity baseline/archive/dedup attribution control failed')
    for field in ('bytes', 'allocated'):
        defective = {str(base / 'catalog-baseline-freeze-01/file'):
                     {'inode': (1, 0), 'bytes': 0, 'allocated': 0, field: 768 * 1024**2 + 1}}
        if capacity_mandatory_subset(defective, base)['status'] != 'confirmed allowance violation':
            raise RuntimeError('capacity allowance violation escaped detection')
    correct = {'memory.max': str(20 * 1024**3), 'memory.swap.max': '0'}
    verify_limits(correct)
    for key, wrong in [('memory.max', '1'), ('memory.swap.max', '1')]:
        try:
            verify_limits(dict(correct, **{key: wrong}))
        except RuntimeError:
            pass
        else:
            raise RuntimeError('wrong containment control accepted')
    for observation in [{'unique_inode_bytes': 2, 'allocated_block_bytes': 0},
                        {'unique_inode_bytes': 0, 'allocated_block_bytes': 2}]:
        try:
            enforce_catalog_budget(observation, maximum=1)
        except RuntimeError:
            pass
        else:
            raise RuntimeError('overbudget control accepted')
    with tempfile.TemporaryDirectory(prefix='catalog-inventory-control-',
            dir=os.environ['FABRIC_SCRATCH_ROOT']) as temporary:
        base = Path(temporary)
        legacy = base / 'lab-completion-run-01'
        dataset = base / 'catalog-control'
        dataset.mkdir()
        source = dataset / 'object'
        source.write_bytes(b'abc')
        for relative in ['recovery/failure-catalog-control.tar.gz',
                         'recovery/failure-catalog-control.tar.manifest.json',
                         'coordinator/launcher-receipts/catalog-control.json',
                         'catalog-cleanup-control.json']:
            path = legacy / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.hardlink_to(source)
        report = evidence_inventory(base)
        total = report['catalog_complete_owned_persistent']
        if (total['files'] != 5 or total['unique_inodes'] != 1
                or total['logical_path_bytes'] != 15 or total['unique_inode_bytes'] != 3
                or total['allocated_block_bytes'] != source.stat().st_blocks * 512):
            raise RuntimeError('archive/receipt coverage or inode dedup control failed')
        categories = report['categories']
        for name, count in [('recovery_failure_archives_and_manifests', 2),
                            ('launcher_receipt_files', 1), ('root_cleanup_receipt_files', 1)]:
            if categories[name]['files'] != count:
                raise RuntimeError('inventory category coverage control failed: ' + name)
        for name, content in [('catalog-maintenance-exclusive', b'query'),
                              ('catalog-spill-exclusive', b'capacity')]:
            directory = base / name
            directory.mkdir()
            (directory / 'only').write_bytes(content)
        bounds = evidence_inventory(base)['conservative_lab_upper_bounds']
        if bounds['capacity']['unique_inode_bytes'] != 11 or bounds['query']['unique_inode_bytes'] != 8:
            raise RuntimeError('exclusive subtraction or overhead double charging control failed')
        if bounds['capacity']['unique_inodes'] != 2 or bounds['query']['unique_inodes'] != 2:
            raise RuntimeError('per-lab upper-bound inode dedup control failed')
        (base / 'catalog-spill-exclusive/query-alias').hardlink_to(
            base / 'catalog-maintenance-exclusive/only')
        bounds = evidence_inventory(base)['conservative_lab_upper_bounds']
        if bounds['capacity']['unique_inode_bytes'] != 16 or bounds['query']['unique_inode_bytes'] != 8:
            raise RuntimeError('cross-lab alias must charge shared inode to both')
        target = base / 'catalog-boundary-diagnostic-01/objects'
        target.mkdir(parents=True)
        (target / 'object').write_bytes(b'linked')
        link = base / 'catalog-boundary-pair2-01/objects'
        link.parent.mkdir()
        link.symlink_to(target, target_is_directory=True)
        aliases = {}
        file_records([target, link], base=base, aliases=aliases)
        if aliases[str(link)]['regular_logical_alias_bytes'] != 6:
            raise RuntimeError('owned object alias control failed')
        try:
            file_records([link], base=base)
        except RuntimeError:
            pass
        else:
            raise RuntimeError('uninventoried target accepted')
        for defect in ('unexpected', 'outside', 'missing'):
            if defect == 'unexpected':
                bad = base / 'catalog-unknown/objects'
                bad.parent.mkdir()
                bad.symlink_to(target, target_is_directory=True)
            else:
                bad = link
                bad.unlink()
                destination = base.parent if defect == 'outside' else base / 'missing'
                bad.symlink_to(destination, target_is_directory=True)
            try:
                file_records([target, bad], base=base)
            except (RuntimeError, OSError):
                pass
            else:
                raise RuntimeError('bad object link accepted: ' + defect)
    return {'wrong_memory_and_swap_limits_rejected': True,
        'capacity_baseline_archive_alias_overhead_and_excess_controls': True,
        'length_and_allocated_overbudget_rejected': True,
        'failure_archive_manifest_launcher_cleanup_included': True,
        'five_paths_one_inode_control': True,
        'exclusive_dataset_subtraction_and_uncertain_double_charge': True,
        'known_object_alias_accepted_only_with_independent_target_inventory': True,
        'unexpected_outside_missing_object_links_rejected': True}

def main():
    require_limits()
    control_results = controls()
    data = ROOT / 'docs/experiments/benchmarks/data/lab-completion-run-01'
    archive = data / 'memory/failure-catalog-many-s-64-r-1-plain-p-1.tar'
    for required in [data.parent / 'catalog-baseline-freeze-01',
                     Path(str(archive) + '.gz'), Path(str(archive) + '.manifest.json')]:
        if not required.exists():
            raise RuntimeError('mandatory capacity evidence missing: ' + str(required))
    receipts = [json.loads(path.read_text()) for path in (data / 'coordinator').glob('*/receipt.json')]
    rows = []
    for record in sorted(receipts, key=lambda r: r['started_unix_ns']):
        if not record['id'].startswith('catalog-') or record['state'] == 'running':
            continue
        final = record.get('cgroup_final', {})
        if not final:
            raise RuntimeError('completed catalog job lacks final resource receipt: ' + record['id'])
        limits = record['limits']
        verify_limits(limits)
        events = counters(final['memory.events'])
        cpu = counters(final['cpu.stat'])
        peak_rss, peak_hwm, memory_parts = {}, {}, {}
        resource_path = data / 'coordinator' / record['id'] / 'resources.jsonl.gz'
        with gzip.open(resource_path, 'rt') as stream:
            for line in stream:
                sample = json.loads(line)
                parts = counters(sample['cgroup']['memory.stat'])
                for key in ('anon', 'file', 'kernel'):
                    memory_parts[key] = max(memory_parts.get(key, 0), parts.get(key, 0))
                for process in sample['processes'].values():
                    name = process['name']
                    peak_rss[name] = max(peak_rss.get(name, 0), process['rss_kib'])
                    peak_hwm[name] = max(peak_hwm.get(name, 0), process['hwm_kib'])
        rows.append({'id': record['id'], 'lab': record['lab'], 'stage': record['stage'],
            'state': record['state'], 'exit': record.get('exit'), 'wall_seconds': record['elapsed_s'],
            'whole_job_cpu_seconds': cpu['usage_usec'] / 1e6,
            'whole_job_peak_bytes': int(final['memory.peak']),
            'separate_sampled_memory_component_peaks': memory_parts,
            'sampled_process_rss_kib_by_name': peak_rss,
            'sampled_process_hwm_kib_by_name': peak_hwm,
            'swap_bytes_final': int(final['memory.swap.current']), 'memory_events': events,
            'io_by_device_raw': final['io.stat'],
            'sampled_scratch_peak_bytes': record['peak_sampled_scratch_bytes'],
            'scratch_removed_at_original_completion': record['scratch_removed'],
            'command': record['command']})
    consumed = sum(r.get('elapsed_s', 0) for r in receipts if r['id'].startswith(('catalog-', 'frontier-')))
    inventory = evidence_inventory(data.parent)
    print(json.dumps({'audit_controls': control_results,
        'persistent_evidence_inventory': inventory, 'jobs': rows, 'recorded_frontier_seconds': consumed,
        'frontier_budget_seconds': 14400, 'remaining_recorded_seconds': 14400 - consumed,
        'active_receipts': [r['id'] for r in receipts if r['state'] == 'running'],
        'peak_catalog_job_bytes': max((r['whole_job_peak_bytes'] for r in rows), default=0),
        'catalog_job_cpu_seconds': sum(r['whole_job_cpu_seconds'] for r in rows),
        'limitations': ['Includes builds, grading and supervisor; not exclusive application RSS/CPU.',
                       'Process-name RSS/HWM is the maximum observed individual PID, not a sum of concurrent processes.',
                       'Job elapsed excludes initial source capture; cgroup CPU/peak include it. Do not divide them for utilization.',
                       'Sampled component maxima occur at different times and must not be added.',
                       'Stacked device IO counters must not be summed as independent physical bytes.',
                       'Successful collection does not imply performance nomination.',
                       'Mechanism probes do not exercise Spool acceptance; no acceptance rate is inferred.',
                       'Later failure cleanup is recorded separately; original completion receipts stay unchanged.',
                       'The summary job itself completes after this snapshot.']}, indent=2))


if __name__ == '__main__':
    main()
