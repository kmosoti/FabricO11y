#!/usr/bin/env python3
"""Authenticate and reduce the finite held-density RowSet locality screen."""
import argparse
import hashlib
import json
from pathlib import Path
from pathlib import PurePosixPath
import struct
import sys
import tarfile

import memory_census as mc
from rowset_locality import ARMS, DENSITIES, LAYOUTS, SEEDS, bytes32, fixture

UNIVERSES = (32768, 262144)
PHASES = ('construction', 'and_once', 'or_once', 'top64_once', 'and_batch', 'or_batch',
          'top64_batch', 'serialization', 'conversion_and', 'conversion_or')
ALLOC_FIELDS = ('wall_ns', 'requested_bytes', 'peak_incremental_bytes', 'retained_incremental_bytes')


def read(path):
    return json.loads(path.read_text())


def sha(path):
    return mc.digest(path)


def ratio(numerator, denominator):
    return numerator / denominator if denominator else None


def decode_u32(raw, n):
    if len(raw) % 4:
        raise ValueError('u32 fixture length is not aligned')
    values = [value[0] for value in struct.iter_unpack('<I', raw)]
    if any(value >= n for value in values) or values != sorted(set(values)):
        raise ValueError('fixture input is not canonical in-range sorted IDs')
    return values


def exact(raw, expected):
    if raw != bytes32(expected):
        raise ValueError('retained native result differs from exact Python set result')


def classify_archive_member(name, is_file, is_dir, seen):
    normalized = name[:-1] if is_dir and name.endswith('/') else name
    path = PurePosixPath(normalized)
    if (not normalized or normalized.endswith('/') or path.is_absolute()
            or '..' in path.parts or str(path) != normalized):
        raise ValueError('native output archive has noncanonical or unsafe member path')
    if normalized == 'output':
        if not is_dir or normalized in seen:
            raise ValueError('native output archive root directory is invalid or duplicated')
        seen.add(normalized)
        return None
    if not is_file or len(path.parts) != 2 or path.parts[0] != 'output' or normalized in seen:
        raise ValueError('native output archive contains unsafe/duplicate member')
    seen.add(normalized)
    return path.parts[1]


def archive_path_controls():
    seen = set()
    if classify_archive_member('output/', False, True, seen) is not None:
        raise ValueError('valid archive directory control rejected')
    if classify_archive_member('output/and.u32', True, False, seen) != 'and.u32':
        raise ValueError('valid archive file control rejected')
    rejected = []
    cases = {
        'traversal': ('output/../escape', True, False, set()),
        'absolute': ('/output/and.u32', True, False, set()),
        'nested': ('output/nested/and.u32', True, False, set()),
        'noncanonical': ('output//and.u32', True, False, set()),
        'root_as_file': ('output', True, False, set()),
        'symlink': ('output/and.u32', False, False, set()),
        'duplicate': ('output/and.u32', True, False, {'output', 'output/and.u32'}),
        'duplicate_root': ('output', False, True, {'output'}),
    }
    for label, (name, is_file, is_dir, prior) in cases.items():
        try:
            classify_archive_member(name, is_file, is_dir, set(prior))
        except ValueError:
            rejected.append(label)
        else:
            raise ValueError('archive path control accepted representative defect: ' + label)
    return {'valid_directory_and_file_accepted': True, 'rejected': rejected}


def read_outputs(cell, row, expected):
    archive_path = cell / 'exact-output.tar.gz'
    if sha(archive_path) != row['archive_sha256']:
        raise ValueError('native output archive digest differs from cell summary')
    observed = {}
    payloads = {}
    seen = set()
    allowed_files = set(expected) | {'a.native', 'b.native'}
    with tarfile.open(archive_path, 'r:gz') as archive:
        for member in archive:
            name = classify_archive_member(member.name, member.isfile(), member.isdir(), seen)
            if name is None:
                continue
            if name not in allowed_files:
                raise ValueError('native output archive contains an unregistered file')
            stream = archive.extractfile(member)
            raw = stream.read(2 * mc.MIB + 1)
            if len(raw) > 2 * mc.MIB:
                raise ValueError('native output member exceeds reducer bound')
            observed[name] = {'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest()}
            payloads[name] = raw
    members = read(cell / 'members.json')
    expected_members = allowed_files
    if ('output' not in seen or members != observed or set(payloads) != expected_members):
        raise ValueError('native output member inventory/readback differs')
    if len(payloads['a.native']) + len(payloads['b.native']) != row['metrics']['native_serialized_bytes']:
        raise ValueError('native serialized byte count differs from retained input artifacts')
    for name, values in expected.items():
        exact(payloads[name], values)
    return observed


def controls_check(controls):
    if not controls['valid_accepted'] or set(controls['rejected']) != {'missing', 'extra', 'order'}:
        raise ValueError('exact output controls incomplete')
    if (not controls.get('valid_mapping_accepted')
            or set(controls.get('mapping_rejected', [])) != {
                'duplicate', 'negative', 'out_of_range', 'wrong_inverse', 'wrong_logical_recovery'}):
        raise ValueError('mapping checker controls incomplete')
    if (not controls.get('known_license_alias_target_validated')
            or set(controls.get('license_alias_controls_rejected', [])) != {
                'wrong_link', 'wrong_path', 'absolute_link', 'wrong_package', 'absent_target'}):
        raise ValueError('license alias controls incomplete')


def main():
    mc.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    source, output = args.source.resolve(), args.out.resolve()
    if not source.is_dir() or output.exists():
        raise ValueError('existing completed source and fresh report output required')
    complete = read(source / 'complete.json')
    if complete.get('exit_code') != 0 or complete.get('native_cells') != 48 or not complete.get('scratch_removed'):
        raise ValueError('locality grid incomplete or owned scratch remains')
    controls = read(source / 'negative-controls.json')
    controls_check(controls)
    archive_controls = archive_path_controls()
    metadata = read(source / 'metadata.json')
    if (metadata.get('qualification') is not False
            or metadata['runner_sha256'] != sha(source / 'rowset_locality.py')
            or metadata['probe_sha256'] != sha(source / 'rowset_probe.rs')
            or metadata['protocol_sha256'] != sha(source / 'protocol.txt')):
        raise ValueError('retained runner, probe, proposal, or qualification metadata mismatch')

    summary = read(source / 'summary.json')
    rows = summary.get('cells')
    if not isinstance(rows, list) or len(rows) != 48 or summary.get('qualification') is not False:
        raise ValueError('summary count or qualification metadata mismatch')
    expected_keys = {(n, density, layout, seed, arm)
                     for n in UNIVERSES for density, _ in DENSITIES for layout in LAYOUTS
                     for seed in SEEDS for arm in ARMS}
    cells = {}
    fixture_receipts = {}
    for row in rows:
        key = row['universe'], row['density'], row['layout'], row['seed'], row['arm']
        if key in cells or key not in expected_keys:
            raise ValueError('duplicate or unregistered summary cell')
        if not row.get('oracle_exact') or row['process'].get('exit_code') != 0:
            raise ValueError('cell failed process or exact-output oracle')
        n, density, layout, seed, arm = key
        divisor = dict(DENSITIES)[density]
        av, bv, generated = fixture(n, divisor, layout, seed)
        fixture_dir = source / f'n-{n}-{density}-{layout}-seed-{seed}'
        fixture_path = fixture_dir / 'fixture.json'
        fixture_meta = read(fixture_path)
        for field in ('universe', 'density', 'k', 'layout', 'seed', 'mapping_sha256_u32le',
                      'logical_cardinalities', 'mapped_cardinalities', 'inverse_membership_validated'):
            if fixture_meta.get(field) != generated.get(field):
                raise ValueError('fixture mapping/cardinality metadata mismatch')
        if (fixture_meta.get('density_label') != density
                or fixture_meta.get('input_a_bytes') != len(bytes32(av))
                or fixture_meta.get('input_b_bytes') != len(bytes32(bv))):
            raise ValueError('fixture density/input size metadata mismatch')
        input_a_path, input_b_path = fixture_dir / 'a.u32', fixture_dir / 'b.u32'
        raw_a, raw_b = input_a_path.read_bytes(), input_b_path.read_bytes()
        actual_a, actual_b = decode_u32(raw_a, n), decode_u32(raw_b, n)
        if actual_a != av or actual_b != bv:
            raise ValueError('retained canonical inputs differ from registered mapped logical sets')
        set_a, set_b = set(actual_a), set(actual_b)
        expected = {'and.u32': sorted(set_a & set_b), 'or.u32': sorted(set_a | set_b),
                    'top64-desc.u32': sorted(set_a & set_b, reverse=True)[:64]}
        metrics = row['metrics']
        if (metrics.get('representation') != arm or metrics.get('universe') != n
                or metrics.get('batch_iterations') != 128):
            raise ValueError('native result identity or operation batch mismatch')
        actual_cardinalities = {'a': metrics.get('input_a_cardinality'),
                                'b': metrics.get('input_b_cardinality'),
                                'and': metrics.get('and_cardinality'),
                                'or': metrics.get('or_cardinality')}
        expected_cardinalities = {'a': len(set_a), 'b': len(set_b),
                                  'and': len(set_a & set_b), 'or': len(set_a | set_b)}
        if actual_cardinalities != expected_cardinalities:
            raise ValueError('native cardinalities differ from independently recomputed input sets')
        if row.get('mapping_sha256_u32le') != fixture_meta['mapping_sha256_u32le']:
            raise ValueError('cell mapping hash differs from fixture record')
        cell_path = fixture_dir / arm
        members = read_outputs(cell_path, row, expected)
        cells[key] = row
        fixture_receipts[f'{n}/{density}/{layout}/{seed}/{arm}'] = {
            'fixture_sha256': sha(fixture_path), 'a_sha256': hashlib.sha256(raw_a).hexdigest(),
            'b_sha256': hashlib.sha256(raw_b).hexdigest(),
            'mapping_sha256_u32le': fixture_meta['mapping_sha256_u32le'],
            'cardinalities': expected_cardinalities, 'native_members_sha256': members}
    if set(cells) != expected_keys:
        raise ValueError('summary does not cover the exact 48-cell grid')

    paired = []
    for n in UNIVERSES:
        for density, _ in DENSITIES:
            for seed in SEEDS:
                for arm in ARMS:
                    contiguous = cells[n, density, 'contiguous', seed, arm]
                    shuffled = cells[n, density, 'shuffled', seed, arm]
                    c, s = contiguous['metrics'], shuffled['metrics']
                    if (c['input_a_cardinality'], c['input_b_cardinality'], c['and_cardinality'], c['or_cardinality']) != (
                            s['input_a_cardinality'], s['input_b_cardinality'], s['and_cardinality'], s['or_cardinality']):
                        raise ValueError('paired layouts do not preserve actual cardinalities')
                    phase_ratios = {}
                    for phase in PHASES:
                        phase_ratios[phase] = {field: ratio(s[phase][field], c[phase][field])
                                               for field in ALLOC_FIELDS}
                    paired.append({
                        'universe': n, 'density': density, 'seed': seed, 'representation': arm,
                        'cardinalities': {'a': c['input_a_cardinality'], 'b': c['input_b_cardinality'],
                                          'and': c['and_cardinality'], 'or': c['or_cardinality']},
                        'layout_ratio': 'shuffled / contiguous', 'phase_ratios': phase_ratios,
                        'native_serialized_bytes_ratio': ratio(s['native_serialized_bytes'], c['native_serialized_bytes']),
                        'constructed_input_pair_bytes_ratio': ratio(
                            s['construction']['retained_incremental_bytes'],
                            c['construction']['retained_incremental_bytes']),
                        'native_postexec_hwm_kib_ratio': ratio(s['native_postexec_hwm_kib'], c['native_postexec_hwm_kib']),
                        'whole_process_wall_seconds_ratio': ratio(shuffled['process']['wall_seconds'], contiguous['process']['wall_seconds']),
                        'whole_process_cpu_seconds_ratio': ratio(
                            shuffled['process']['user_seconds'] + shuffled['process']['system_seconds'],
                            contiguous['process']['user_seconds'] + contiguous['process']['system_seconds']),
                        'contiguous_mapping_sha256_u32le': contiguous['mapping_sha256_u32le'],
                        'shuffled_mapping_sha256_u32le': shuffled['mapping_sha256_u32le'],
                    })
    result = {
        'qualification': False,
        'interpretation': 'paired physical-arrangement screen at fixed logical density/cardinality/overlap; no performance threshold or Fabric claim',
        'cell_count': len(cells), 'paired_layout_count': len(paired), 'paired_layouts': paired,
        'fixture_receipts': fixture_receipts, 'archive_path_controls': archive_controls,
        'input_receipt_sha256': {name: sha(source / name) for name in
                                 ('summary.json', 'complete.json', 'negative-controls.json', 'metadata.json')},
        'runner_sha256': metadata['runner_sha256'], 'probe_sha256': metadata['probe_sha256'],
        'proposal_sha256': metadata['protocol_sha256'], 'reporter_sha256': sha(Path(__file__)),
        'limitations': [
            'two deterministic seeds and four paired layout cases per universe/density/representation',
            'generated ID arrangements are physical controls, not representative Fabric rows or storage',
            'operation batches reuse constructed inputs; phase values include the registered allocator instrumentation',
            'native VmHWM includes fixture decoding, conversion, serialization, and correctness export',
            'ratios describe these matched pairs and have no performance acceptance threshold',
        ],
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'report': str(output), 'cells': len(cells), 'paired_layouts': len(paired),
                      'winner': 'not claimed'}))


if __name__ == '__main__':
    main()
