#!/usr/bin/env python3
"""Read-only matched-query/Vortex evidence report; run through resource_group.py."""
import argparse
import gzip
import hashlib
import json
import math
from pathlib import Path
import statistics
import sys
import tarfile

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits

NAMES = ('absent', 'selective', 'rare', 'common', 'all', 'narrow-time')
QUERY_ARMS = ('arrow-full', 'arrow-pruned', 'duckdb-parquet', 'duckdb-table')
VORTEX_ARMS = ('arrow-parquet-full', 'vortex-full', 'vortex-predicate')
METRICS = ('combined_wall_ns', 'combined_cpu_ns')


def read(path):
    return json.loads(path.read_text())


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def digest_check(raw, digest, size=None):
    if hashlib.sha256(raw).hexdigest() != digest or size is not None and len(raw) != size:
        raise RuntimeError('decoded artifact hash/length mismatch')


def object_path(directory, digest):
    if len(digest) != 64 or any(c not in '0123456789abcdef' for c in digest):
        raise RuntimeError('invalid object digest')
    path = directory / digest
    if path.is_symlink() or not path.is_file() or sha(path) != digest:
        raise RuntimeError('retained object missing, linked or changed')
    return path


def decompress(path, cap=64 * 1024**2):
    with gzip.open(path, 'rb') as stream:
        raw = stream.read(cap + 1)
    if len(raw) > cap:
        raise RuntimeError('decoded evidence exceeds bounded report admission')
    return raw


def coverage(rows, arms):
    expected = {(a, n, i) for a in arms for n in NAMES for i in range(4)}
    seen = set()
    for row in rows:
        name = row['query']['name'] if isinstance(row['query'], dict) else row['query']
        key = row['arm'], name, row['iteration']
        if key in seen or key not in expected:
            raise RuntimeError('duplicate/unregistered measurement or verdict')
        seen.add(key)
    if seen != expected:
        raise RuntimeError('measurement/verdict coverage mismatch')


def controls():
    rows = [{'arm': a, 'query': n, 'iteration': i} for a in QUERY_ARMS for n in NAMES for i in range(4)]
    coverage(rows, QUERY_ARMS)
    digest_check(b'valid', hashlib.sha256(b'valid').hexdigest(), 5)
    rejected = []
    for name, bad in [('missing_measurement', rows[:-1]), ('duplicate_measurement', rows + rows[:1]),
                      ('unknown_arm', [dict(rows[0], arm='unregistered'), *rows[1:]])]:
        try:
            coverage(bad, QUERY_ARMS)
        except RuntimeError:
            rejected.append(name)
        else:
            raise RuntimeError('report checker accepted representative coverage defect')
    try:
        digest_check(b'valie', hashlib.sha256(b'valid').hexdigest(), 5)
    except RuntimeError:
        rejected.append('changed_decoded_answer')
    else:
        raise RuntimeError('report checker accepted changed answer')
    return {'valid_controls_checked': True, 'rejected': rejected}


def expected_query(name):
    end = 32768 // 2
    needle = {'absent': 'MISSING', 'selective': 'S!x', 'rare': 'R?y', 'common': 'C%z', 'all': '', 'narrow-time': ''}[name]
    return {'name': name, 'from': end // 2 if name == 'narrow-time' else 0,
            'to': end // 2 + end // 16 if name == 'narrow-time' else end, 'contains': needle}


def source_expectation(rows, query):
    matches = [row for row in rows if query['from'] <= row[5] < query['to'] and query['contains'] in row[6]]
    matches.sort(key=lambda row: (row[5], row[2], row[3], row[4]))
    return matches[:64], len(matches)


def archive_provenance(root):
    manifest = read(root / 'source-manifest.json')
    seen = set()
    with tarfile.open(root / 'sources.tar.gz', 'r:gz') as archive:
        for member in archive:
            name = member.name
            if not member.isfile() or name in seen or name not in manifest or Path(name).is_absolute() or '..' in Path(name).parts:
                raise RuntimeError('source archive type/path/coverage defect')
            raw = archive.extractfile(member).read(2 * 1024**2 + 1)
            digest_check(raw, manifest[name]['sha256'], manifest[name]['bytes'])
            seen.add(name)
    if seen != set(manifest):
        raise RuntimeError('source archive missing member')
    return {'archive_sha256': sha(root / 'sources.tar.gz'), 'manifest_sha256': sha(root / 'source-manifest.json'),
            'exact_archived_members_verified': len(seen),
            'current_working_tree_not_substituted_for_historical_sources': True}


def median_phases(rows):
    result = {}
    for row in rows:
        phases = row.get('phases') or {name: row[name] for name in ('native_query', 'python_result_projection')}
        for name, phase in phases.items():
            if phase is not None:
                for metric in ('wall_ns', 'process_cpu_ns'):
                    result.setdefault(name, {}).setdefault(metric, []).append(phase[metric])
    return {name: {m: statistics.median(values) for m, values in metrics.items()} for name, metrics in result.items()}


def report_lab(root, vortex=False):
    cleanup = read(root / 'cleanup.json')
    if cleanup['status'] != 'passed' or not cleanup['removed']:
        raise RuntimeError('completed clean experiment receipt required')
    arms = VORTEX_ARMS if vortex else QUERY_ARMS
    environment = read(root / 'environment.json')
    if not 1 <= len(environment['cpu_affinity']) <= 2:
        raise RuntimeError('recorded CPU affinity exceeds registered two-CPU bound')
    pins = {pin[0]: pin[1] for pin in environment['wheel_pins']}
    for name, version, filename, base, digest, size in environment['wheel_pins']:
        wheel_receipt = read(root / (name + ('-wheel.json' if vortex else '-wheel-verification.json')))
        if not wheel_receipt['verified'] or wheel_receipt['sha256'] != digest or wheel_receipt['bytes'] != size:
            raise RuntimeError('wheel receipt differs from frozen pin')
    inventory = read(root / 'results.json')
    if len(inventory) != (8 if vortex else 16):
        raise RuntimeError('fresh-child inventory count differs from registered sweep')
    source_cache, answer_cache = {}, {}
    populations, constructions, repayment, children, controls_count = [], [], [], [], {'checker_rejections': 0, 'actual_index_rejections': 0,
        'actual_wildcard_rejections': 0, 'engine_literal_checks': 0}
    identities, children_seen = {}, set()
    for item in inventory:
        cell = item['cell']
        key = cell['width'], cell['row_group'], cell['locality'], cell['repeat']
        if key in children_seen or cell['records'] != 32768 or cell['seed'] != 42 or cell['repeat'] not in (0, 1):
            raise RuntimeError('duplicate/unregistered child fixture')
        children_seen.add(key)
        label = (f'b{cell["width"]}-{cell["locality"]}-r{cell["repeat"]}' if vortex else
                 f'b{cell["width"]}-g{cell["row_group"]}-{cell["locality"]}-r{cell["repeat"]}')
        evidence = root / label
        result, measurements, verdicts = read(evidence / 'result.json'), read(evidence / 'measurements.json'), read(evidence / 'verdicts.json')
        if result['versions'] != pins:
            raise RuntimeError('installed distribution versions differ from frozen wheel pins')
        coverage(measurements, arms)
        coverage(verdicts, arms)
        if vortex:
            source_digest, parquet_digest = cell['source_sha256'], cell['parquet_sha256']
            source_file = object_path(root / 'inputs', source_digest)
            object_path(root / 'inputs', parquet_digest)
            answer_receipts = read(evidence / 'answer-objects.json')
            if result['representation']['kernel_dispatch_verified']:
                raise RuntimeError('unregistered FSST dispatch assertion')
        else:
            inputs = read(evidence / 'input-objects.json')
            if inputs != item['objects']:
                raise RuntimeError('input map differs from run inventory')
            for digest in inputs.values():
                object_path(root / 'objects', digest)
            source_digest, parquet_digest = inputs['source.jsonl.gz'], inputs['logs.parquet']
            source_file = root / 'objects' / source_digest
            answer_receipts = read(evidence / 'answer-objects.json')
            identity_key = key[:3]
            if identity_key in identities and identities[identity_key] != inputs:
                raise RuntimeError('fresh process fixture/index identity differs')
            identities[identity_key] = inputs
            injected = read(evidence / 'group-pruning-controls.json')
            if len(injected) != 2 or {c['defect'] for c in injected} != {'omit_matching_group', 'corrupt_matching_trigram'}:
                raise RuntimeError('actual pruning control coverage differs')
        if source_digest not in source_cache:
            logical = [json.loads(line) for line in decompress(source_file).splitlines()]
            if len(logical) != 32768 or any(len(row) != 8 or len(row[6].encode()) != cell['width'] for row in logical):
                raise RuntimeError('source fixture row schema/count/body width differs')
            source_cache[source_digest] = logical
        logical = source_cache[source_digest]
        expectations = {name: source_expectation(logical, expected_query(name)) for name in NAMES}
        if vortex:
            bad = result['wildcard_control']
            if not bad['raw_like_rejected'] or not bad['exact_fallback_passed'] or bad['raw_like_rows'] == bad['expected_literal_rows']:
                raise RuntimeError('actual wildcard defect not rejected/restored')
            controls_count['actual_wildcard_rejections'] += 1
        else:
            for control in injected:
                if (not control['rejected'] or control['expected_rows'] != expectations['selective'][0]
                        or control['actual_rows'] == control['expected_rows'] or control['group'] in control['selected_groups']):
                    raise RuntimeError('actual index defect failed source-based rejection check')
            controls_count['actual_index_rejections'] += 2
        if len(result['checker_controls']['rejected']) != 6 or not result['checker_controls']['valid_literal_control']:
            raise RuntimeError('independent answer checker control coverage differs')
        controls_count['checker_rejections'] += 6
        literals = result['literal_controls']
        if len(literals) != (21 if vortex else 12) or not all(c['passed'] for c in literals):
            raise RuntimeError('engine literal control coverage differs')
        controls_count['engine_literal_checks'] += len(literals)
        by_verdict = {(v['arm'], v['query'], v['iteration']): v for v in verdicts}
        for row in measurements:
            name = row['query']['name']
            if row['query'] != expected_query(name) or row['matching_rows'] != expectations[name][1]:
                raise RuntimeError('query contract/selectivity drift')
            v = by_verdict[row['arm'], name, row['iteration']]
            if not v['passed']:
                raise RuntimeError('recorded semantic verdict failed')
            digest = v['answer_raw_sha256']
            receipt = answer_receipts[digest]
            compressed_digest = receipt if vortex else receipt['object_sha256']
            path = object_path(root / 'objects', compressed_digest)
            if digest not in answer_cache:
                raw = decompress(path, 2 * 1024**2)
                digest_check(raw, digest, v['answer_raw_bytes'])
                answer_cache[digest] = json.loads(raw)
            if answer_cache[digest] != expectations[name][0]:
                raise RuntimeError('retained actual answer differs from independently filtered source')
        process = read(evidence / 'process.json')
        if process['exit'] != 0:
            raise RuntimeError('child process receipt not successful')
        children.append({'cell': cell, 'process': process, 'versions': result['versions'],
                         'result_sha256': sha(evidence / 'result.json'), 'source_sha256': source_digest,
                         'parquet_sha256': parquet_digest, 'measurement_count': len(measurements),
                         'representation': result.get('representation'), 'limitations': result.get('limitations')})
        construction = {'cell': cell, 'costs': result['construction'], 'row_groups': result.get('row_groups')}
        constructions.append(construction)
        for query in NAMES:
            for arm in arms:
                for boundary, iterations in (('first', (0,)), ('reuse_median', (1, 2, 3))):
                    rows = [m for m in measurements if m['arm'] == arm and m['query']['name'] == query and m['iteration'] in iterations]
                    entry = {'cell': cell, 'arm': arm, 'query': query, 'boundary': boundary, 'samples': len(rows),
                        **{metric: statistics.median(m[metric] for m in rows) for metric in METRICS},
                        'phase_medians': median_phases(rows), 'matching_rows': expectations[query][1]}
                    if not vortex and arm.startswith('arrow'):
                        candidates = {tuple(m['selected_row_groups']) for m in rows}
                        if len(candidates) != 1:
                            raise RuntimeError('same-query candidate groups vary unexpectedly')
                        groups = next(iter(candidates))
                        size = sum(result['row_groups'][i]['compressed_column_bytes'] for i in groups)
                        if any(m['candidate_compressed_column_bytes'] != size for m in rows):
                            raise RuntimeError('candidate byte accounting differs from actual metadata ledger')
                        entry.update(selected_groups=list(groups), candidate_compressed_column_bytes=size,
                            total_groups=len(result['row_groups']), candidate_fraction=len(groups) / len(result['row_groups']))
                    if vortex:
                        entry['routes'] = sorted({m['route'] for m in rows})
                    populations.append(entry)
        models = ((('arrow-full', 'arrow-pruned', ('exact_trigram_index_build', 'index_serialize_write_readback')),
                   ('duckdb-parquet', 'duckdb-table', ('duckdb_ingest', 'duckdb_checkpoint'))) if not vortex else
                  tuple(('arrow-parquet-full', candidate, ('parquet_decode_for_conversion',
                    'fixed_binary_to_binary_conversion', 'vortex_compress_write', 'vortex_open'))
                    for candidate in ('vortex-full', 'vortex-predicate')))
        for baseline, candidate, costs in models:
            for query in NAMES:
                for metric, costmetric in zip(METRICS, ('wall_ns', 'process_cpu_ns')):
                    a = next(p for p in populations if p['cell'] == cell and p['arm'] == baseline and p['query'] == query and p['boundary'] == 'reuse_median')
                    b = next(p for p in populations if p['cell'] == cell and p['arm'] == candidate and p['query'] == query and p['boundary'] == 'reuse_median')
                    cost = sum(result['construction'][c][costmetric] for c in costs)
                    saving = a[metric] - b[metric]
                    repayment.append({'cell': cell, 'baseline': baseline, 'candidate': candidate, 'query': query,
                        'metric': metric, 'charged_cost_names': list(costs), 'incremental_cost_ns': cost,
                        'reuse_median_saving_ns': saving, 'queries_to_repay': math.ceil(cost / saving) if saving > 0 else None})
    expected_cells = {(w, g, loc, rep) for w in (16, 1024) for g in ((8192,) if vortex else (1024, 8192))
                      for loc in ('clustered', 'mixed') for rep in (0, 1)}
    if children_seen != expected_cells:
        raise RuntimeError('configuration inventory differs from registered cells')
    paired = []
    for p in populations:
        baseline_arm = arms[0]
        if p['arm'] == baseline_arm:
            continue
        baseline = next(a for a in populations if a['cell'] == p['cell'] and a['arm'] == baseline_arm
                        and a['query'] == p['query'] and a['boundary'] == p['boundary'])
        paired.append({'cell': p['cell'], 'baseline': baseline_arm, 'candidate': p['arm'],
            'query': p['query'], 'boundary': p['boundary'],
            **{metric: {'baseline': baseline[metric], 'candidate': p[metric],
                'candidate_over_baseline': p[metric] / baseline[metric]} for metric in METRICS}})
    locality, group_size_effect = [], []
    for p in populations:
        if p['cell']['locality'] != 'mixed':
            continue
        cluster_cell = dict(p['cell'], locality='clustered')
        # Vortex source/parquet pointers differ by locality; use the physical cell dimensions.
        counterpart = next(a for a in populations if a['cell']['width'] == cluster_cell['width']
            and a['cell']['row_group'] == cluster_cell['row_group'] and a['cell']['repeat'] == cluster_cell['repeat']
            and a['cell']['locality'] == 'clustered' and a['arm'] == p['arm'] and a['query'] == p['query']
            and a['boundary'] == p['boundary'])
        locality.append({'width': p['cell']['width'], 'row_group': p['cell']['row_group'], 'repeat': p['cell']['repeat'],
            'arm': p['arm'], 'query': p['query'], 'boundary': p['boundary'],
            **{m: {'clustered': counterpart[m], 'mixed': p[m], 'mixed_over_clustered': p[m] / counterpart[m]} for m in METRICS},
            'clustered_candidate_fraction': counterpart.get('candidate_fraction'), 'mixed_candidate_fraction': p.get('candidate_fraction')})
    if not vortex:
        for p in populations:
            if p['cell']['row_group'] != 8192:
                continue
            counterpart = next(a for a in populations if a['cell']['width'] == p['cell']['width']
                and a['cell']['row_group'] == 1024 and a['cell']['repeat'] == p['cell']['repeat']
                and a['cell']['locality'] == p['cell']['locality'] and a['arm'] == p['arm']
                and a['query'] == p['query'] and a['boundary'] == p['boundary'])
            group_size_effect.append({'width': p['cell']['width'], 'locality': p['cell']['locality'],
                'repeat': p['cell']['repeat'], 'arm': p['arm'], 'query': p['query'], 'boundary': p['boundary'],
                **{m: {'g1024': counterpart[m], 'g8192': p[m], 'g8192_over_g1024': p[m] / counterpart[m]} for m in METRICS},
                'g1024_candidate_fraction': counterpart.get('candidate_fraction'), 'g8192_candidate_fraction': p.get('candidate_fraction')})
    return {'root': str(root), 'children': children, 'measurements': sum(c['measurement_count'] for c in children),
        'exact_answers_rechecked_against_source': sum(c['measurement_count'] for c in children),
        'controls': controls_count, 'provenance': archive_provenance(root), 'cleanup': cleanup,
        'environment': {k: environment[k] for k in ('python', 'uname', 'cpu_affinity', 'wheel_pins',
            'revision', 'versions', 'uv', 'limits', 'installer') if k in environment},
        'populations': populations, 'paired_medians': paired, 'locality_effect': locality,
        'row_group_size_effect': group_size_effect,
        'construction': constructions, 'repayment_models': repayment,
        'limitations': ['First/reuse are process-lifetime boundaries, not OS cold/warm caches.',
            'Two fresh repetitions are retained separately; no pooled acceptance or engine dominance.',
            'Candidate compressed bytes are metadata sums, not measured physical IO or decoded bytes.',
            'Library boundaries and Python result projection differ; inspect phase totals before cross-engine interpretation.',
            'Repayment is fixed-query incremental construction arithmetic, not refill/update or steady-state measurement.',
            'Historical CR2 no-nomination remains; no production format migration or deployment qualification.']}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--query', type=Path)
    parser.add_argument('--vortex', type=Path)
    parser.add_argument('--out', type=Path)
    parser.add_argument('--controls', action='store_true')
    args = parser.parse_args()
    require_limits()
    checked = controls()
    if args.controls:
        print(json.dumps(checked))
        return
    if not args.out or not (args.query or args.vortex):
        parser.error('--out and at least one input lab required')
    if args.out.exists():
        raise RuntimeError('fresh report path required; historical reports remain immutable')
    report = {'report_controls': checked}
    if args.query:
        report['query'] = report_lab(args.query.resolve())
    if args.vortex:
        report['vortex'] = report_lab(args.vortex.resolve(), vortex=True)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, indent=2, ensure_ascii=False) + '\n')
    print(json.dumps({'report': str(args.out), 'labs': {name: {'children': len(value['children']),
        'measurements': value['measurements'], 'controls': value['controls']} for name, value in report.items() if name != 'report_controls'}}))


if __name__ == '__main__':
    main()
