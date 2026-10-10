#!/usr/bin/env python3
"""Independent, compact accounting audit for one archived dev-small cell.

This validator consumes retained observations only. It does not launch a
workload or call the legacy aggregate checker.
"""
from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import math
import os
from pathlib import Path
import re
import tempfile
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits

PHASES = (('normal', 0), ('burst', 60), ('recovery', 120))


def digest(path: Path) -> str:
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()


def json_file(path: Path):
    return json.loads(path.read_text())


def json_gzip_or_plain(path: Path):
    opener = gzip.open if path.suffix == '.gz' else open
    with opener(path, 'rt') as stream:
        return json.load(stream)


def rows(path: Path):
    opener = gzip.open if path.suffix == '.gz' else open
    with opener(path, 'rt') as stream:
        for line_no, line in enumerate(stream, 1):
            try:
                yield json.loads(line)
            except Exception as exc:
                raise ValueError(f'{path.name}:{line_no}: invalid JSON: {exc}') from exc


def nearest(values, fraction):
    values = sorted(values)
    if not values:
        return None
    return values[max(0, math.ceil(fraction * len(values)) - 1)]


def read_events(root: Path):
    result = []
    for path in sorted(root.glob('node*-events.jsonl.gz')):
        match = re.fullmatch(r'node([0-9]+)-events\.jsonl\.gz', path.name)
        if not match:
            raise ValueError(f'unexpected event filename: {path.name}')
        label = 'node' + match.group(1)
        for event in rows(path):
            result.append((label, event))
    return result


def audit_data(root: Path):
    problems: list[str] = []
    def require(ok, reason):
        if not ok:
            problems.append(reason)

    summary = json_file(root / 'summary.json')
    def existing(*names):
        for name in names:
            path = root / name
            if path.exists():
                return path
        raise FileNotFoundError(f'none of {names!r} exists in {root}')
    recovered_rows = list(rows(existing('recovered-hashes.jsonl.gz', 'recovered-hashes.jsonl')))
    recovered = {}
    for row in recovered_rows:
        if len(row) < 5:
            raise ValueError('recovered-hashes row must have five fields')
        label, seq, sha, size, received = row[:5]
        key = (str(label), int(seq))
        require(key not in recovered, f'duplicate recovered key {key}')
        recovered[key] = (str(sha), int(size), int(received))

    cycles, acks = {}, {}
    events = read_events(root)
    for label, event in events:
        kind = event.get('kind')
        if kind == 'cycle':
            key = (label, int(event['batch']))
            require(key not in cycles, f'duplicate live cycle {key}')
            cycles[key] = event
        elif kind == 'ack_attempt' and event.get('status') == 'ack':
            key = (label, int(event['sequence']))
            acks.setdefault(key, event)

    # Seed history has its own custody boundary. It is deliberately not
    # expected to have live cycle/ACK stdout observations.
    seed_rows = list(rows(existing('seed-ledger.jsonl.gz', 'seed-ledger.jsonl')))
    seed = {}
    seed_source_hashes = {}
    for row in seed_rows:
        key = (str(row['label']), int(row['sequence']))
        require(key not in seed, f'duplicate seed key {key}')
        seed[key] = (str(row['sha256']), int(row['bytes']))
        for tag, body_sha in row.get('source', []):
            require(tag not in seed_source_hashes, f'duplicate seeded source {tag}')
            seed_source_hashes[tag] = body_sha
    seed_summary = json_file(root / 'seed-summary.json')
    seed_verify = json_file(root / 'seed-verification.json')
    condition = seed_summary.get('condition')
    require(condition in ('H0', 'H256', 'nearrotation'), 'unknown seed condition')
    require(sum(n for _, n in seed.values()) == int(seed_summary.get('encoded_bytes', -1)),
            'seed encoded-byte total')
    require(len(seed) == int(seed_summary.get('batches', -1)), 'seed batch total')
    require(len(seed_source_hashes) == int(seed_summary.get('rows', -1)), 'seed source-row total')
    require(bool(seed_verify.get('seed_replay_exact')), 'seed fixture replay was not exact')
    require(int(seed_verify.get('seed_batch_hash_mismatches', -1)) == 0,
            'seed Batch hash mismatch count')
    require(bool(seed_verify.get('seed_body_fidelity_in_exact_source_gate')),
            'seed body fidelity was not included in exact-source grading')
    require(int(seed_verify.get('seed_batches', -1)) == len(seed), 'seed verification batch census')
    require(int(seed_verify.get('seed_source_rows', -1)) == len(seed_source_hashes),
            'seed verification source census')
    require(seed_verify.get('prefix_sha256') == seed_summary.get('prefix_sha256'),
            'seed prefix hash provenance differs')
    for key, (sha, size) in seed.items():
        require(key in recovered, f'seed Batch missing after recovery: {key}')
        if key in recovered:
            require(recovered[key][:2] == (sha, size), f'seed Batch bytes differ: {key}')

    live_recovered = set(recovered) - set(seed)
    require(set(cycles) == set(acks) == live_recovered,
            'live cycle/ACK/recovered identity sets differ')
    for key in set(cycles) & set(acks) & live_recovered:
        require(int(cycles[key]['batch']) == int(acks[key]['sequence']),
                f'cycle/ACK sequence differs: {key}')
        require(str(acks[key].get('sha256', '')) == recovered[key][0],
                f'ACK/recovered encoded Batch hash differs: {key}')
    require(set(recovered) == set(seed) | set(cycles), 'seed/live partitions do not cover recovery')
    by_label = {}
    for label, sequence in recovered:
        by_label.setdefault(label, []).append(sequence)
    for label, sequences in by_label.items():
        ordered = sorted(sequences)
        require(ordered == list(range(1, ordered[-1] + 1)),
                f'noncontiguous recovered sequences for {label}')

    # Source, clocks and recovered identity cross-checks retain enough detail
    # for independent counters without retaining decoded Batch payloads.
    source_rows = list(rows(existing('sources.jsonl.gz', 'sources.jsonl')))
    source = {}
    source_rows_by_tag = {}
    for row in source_rows:
        if len(row) < 5:
            raise ValueError('source row must have tag, hash, source time, phase and lag')
        tag, body_sha = str(row[0]), str(row[1])
        require(tag not in source, f'duplicate offered source tag {tag}')
        source[tag] = body_sha
        source_rows_by_tag[tag] = row
    live_source = source
    require(len(source) + len(seed_source_hashes) == int(summary.get('expected_logs', -1)),
            'live plus seeded source census differs from expected logs')
    clock_rows = list(rows(existing('data-clocks.jsonl.gz', 'data-clocks.jsonl')))
    clock_tags = {}
    for row in clock_rows:
        if len(row) < 8:
            raise ValueError('data-clocks row must have tag, identity, clocks and body SHA-256')
        tag, label, seq = str(row[0]), str(row[1]), int(row[2])
        is_seed = tag in seed_source_hashes
        require(tag in source or is_seed, f'clock row has unknown source tag {tag}')
        expected_body_sha = seed_source_hashes.get(tag, source.get(tag))
        require(str(row[7]) == expected_body_sha, f'recovered source body hash differs for {tag}')
        if is_seed:
            require(row[3] is None, f'historical source unexpectedly has live source clock: {tag}')
        elif tag in source:
            require(int(row[3]) == int(source_rows_by_tag[tag][2]),
                    f'clock/source timestamp differs for {tag}')
        require((label, seq) in recovered, f'clock row references unrecovered Batch {(label, seq)}')
        require(tag not in clock_tags, f'duplicate recovered source clock {tag}')
        clock_tags[tag] = (label, seq)
    require(len(live_source) == len(source_rows), 'source ledger includes unexpected historical rows')
    require(len(clock_tags) == len(live_source) + len(seed_source_hashes),
            'source/seed/recovered clock census differs')

    # Recompute phase CPU and clock validity from raw integer observations.
    samples = json_gzip_or_plain(existing('resources.json.gz', 'resources.json'))
    require(len(samples) >= 2, 'fewer than two raw resource samples')
    epoch = int(summary.get('observation', {}).get('epoch_ns', 0))
    clock = {'samples': len(samples), 'realtime_range_ns': None, 'suspend_range_ns': None,
             'boottime_available': bool(samples) and all(s.get('boot_ns') is not None for s in samples),
             'phases': {}}
    if samples:
        rt = [int(s.get('observer_wall_ns', s['wall_ns'])) -
              int(s.get('observer_mono_ns', s['mono_ns'])) for s in samples]
        bt = [int(s['boot_ns']) - int(s.get('observer_mono_ns', s['mono_ns']))
              for s in samples if s.get('boot_ns') is not None]
        clock['realtime_range_ns'] = max(rt) - min(rt)
        clock['suspend_range_ns'] = max(bt) - min(bt) if bt else None
    for phase, offset in PHASES:
        lo, hi = epoch + offset * 10**9, epoch + (offset + 60) * 10**9
        group = [s for s in samples if lo <= int(s['wall_ns']) < hi]
        if len(group) < 2:
            clock['phases'][phase] = {'samples': len(group), 'server_cpu_cores': None,
                                      'reason': 'fewer than two raw samples'}
            continue
        elapsed = int(group[-1]['mono_ns']) - int(group[0]['mono_ns'])
        cpu = float(group[-1]['processes'][0]['cpu_s']) - float(group[0]['processes'][0]['cpu_s'])
        value = cpu / (elapsed / 1e9) if elapsed > 0 else None
        clock['phases'][phase] = {'samples': len(group), 'server_cpu_cores': value,
                                  'reason': None if value is not None else 'nonpositive monotonic interval'}
    clock['timing_eligible'] = (clock['realtime_range_ns'] is not None and
        clock['realtime_range_ns'] <= 5_000_000 and clock['boottime_available'] and
        clock['suspend_range_ns'] is not None and clock['suspend_range_ns'] <= 5_000_000 and
        all(v['server_cpu_cores'] is not None for v in clock['phases'].values()))
    reported_measurement = summary.get('measurement', {})
    reported_clock = reported_measurement.get('clock', {})
    require(reported_clock.get('realtime_range_ns') == clock['realtime_range_ns'],
            'reported realtime range differs from raw samples')
    require(reported_clock.get('suspend_range_ns') == clock['suspend_range_ns'],
            'reported suspend range differs from raw samples')
    for phase, measured in clock['phases'].items():
        reported = reported_measurement.get('phases', {}).get(phase, {})
        require(int(reported.get('samples', -1)) == measured['samples'],
                f'{phase} reported sample count differs from raw samples')
        actual_cpu, reported_cpu = measured['server_cpu_cores'], reported.get('server_cpu_cores')
        if actual_cpu is None:
            require(reported_cpu is None and bool(reported.get('reason')),
                    f'{phase} missing CPU was not preserved as null with reason')
        else:
            require(isinstance(reported_cpu, (int, float)) and
                    math.isclose(actual_cpu, float(reported_cpu), rel_tol=0, abs_tol=1e-12),
                    f'{phase} reported CPU differs from raw samples')

    # Verify retained artifact hashes where the coordinator manifest lists
    # files. Permit either {relative_path: sha256} or [{path, sha256}, ...].
    manifest = json_file(root / 'artifact-manifest.json')
    entries = manifest.get('files', manifest)
    if isinstance(entries, dict):
        expected_files = entries.items()
    elif isinstance(entries, list):
        expected_files = ((entry['path'], entry['sha256']) for entry in entries)
    else:
        raise ValueError('artifact manifest files must be a map or list')
    manifest_checks = 0
    for relative, expected_sha in expected_files:
        path = (root / relative).resolve()
        require(path.is_relative_to(root.resolve()), f'manifest path escapes cell: {relative}')
        require(path.is_file(), f'manifest file missing: {relative}')
        if path.is_file():
            require(digest(path) == expected_sha, f'artifact hash mismatch: {relative}')
            manifest_checks += 1
    require(manifest_checks > 0, 'artifact manifest has no file hashes')

    # Oracle provenance is compact: the observer retains verdicts and the
    # unchanged Python oracle/replay hashes, not the bulky decoded replay.
    provenance_path = root / 'query-provenance.json'
    provenance = json_file(provenance_path) if provenance_path.exists() else {}
    verdicts = json_file(root / 'query-verdicts.json')
    oracle_path = ROOT / 'tools/qualification/query_oracle.py'
    oracle_sha = digest(oracle_path)
    require(provenance.get('oracle_sha256') == oracle_sha, 'query oracle source hash mismatch')
    require(Path(str(provenance.get('oracle_path', ''))).name == 'query_oracle.py',
            'query oracle path provenance missing')
    require(provenance.get('verdict_file') == 'query-verdicts.json', 'query verdict provenance missing')
    final_verdicts = verdicts.get('final', [])
    controls = verdicts.get('controls', [])
    require(bool(final_verdicts) and all(v.get('passed') for v in final_verdicts),
            'independent final query verdict failed or missing')
    require(len(controls) >= 2 and all(not c.get('verdict', {}).get('passed') for c in controls),
            'query missing/changed negative controls not rejected')
    require(int(provenance.get('quiescent_query_count', -1)) == len(final_verdicts),
            'query provenance verdict count mismatch')
    require(int(provenance.get('negative_control_count', -1)) == len(controls),
            'query provenance negative-control count mismatch')
    replay_sha = provenance.get('recovered_sha256')
    require(isinstance(replay_sha, str) and re.fullmatch(r'[0-9a-f]{64}', replay_sha) is not None,
            'decoded replay hash provenance missing')

    env = json_file(root / 'environment.json')
    require(bool(env), 'environment provenance is empty')
    source_hashes = env.get('sources', {})
    require(isinstance(source_hashes, dict) and bool(source_hashes),
            'source snapshot hash map is missing')
    for relative, expected_sha in source_hashes.items():
        source_path = (ROOT / relative).resolve()
        require(source_path.is_relative_to(ROOT.resolve()), f'source path escapes repository: {relative}')
        require(source_path.is_file(), f'source snapshot file missing: {relative}')
        if source_path.is_file():
            require(digest(source_path) == expected_sha, f'source snapshot hash mismatch: {relative}')
    protocol_path = ROOT / 'docs/experiments/benchmarks/dev-small-labs-screen-protocol.md'
    if env.get('protocol_sha256') is not None:
        require(digest(protocol_path) == env['protocol_sha256'], 'registered protocol hash mismatch')
    return {'problems': problems, 'counts': {'seed_batches': len(seed),
            'recovered_batches': len(recovered), 'live_cycles': len(cycles),
            'live_acks': len(acks), 'source_logs': len(source), 'clock_rows': len(clock_rows),
            'resource_samples': len(samples), 'query_verdicts': len(final_verdicts)},
            'clock': clock, 'query_provenance': {'oracle_path': 'tools/qualification/query_oracle.py',
            'oracle_sha256': oracle_sha, 'replay_sha256': replay_sha,
            'verdict_file': provenance.get('verdict_file'),
            'negative_controls': len(controls)}, 'artifact_hashes_checked': manifest_checks,
            'environment_keys': sorted(env), 'passed': not problems}


def controls():
    """Build a minimal valid cell and require semantic defects to be rejected."""
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    scratch.mkdir(parents=True, exist_ok=True)
    checks = {}
    with tempfile.TemporaryDirectory(prefix='dev-small-audit-control-', dir=scratch) as name:
        root = Path(name)
        epoch, mono0 = 1_700_000_000_000_000_000, 1_699_000_000_000_000_000
        tag, body_sha, batch_sha = '00:0000:00', hashlib.sha256(b'body').hexdigest(), hashlib.sha256(b'batch').hexdigest()
        def put_json(path, value):
            (root / path).write_text(json.dumps(value, separators=(',', ':')) + '\n')
        def put_lines(path, values):
            with gzip.open(root / path, 'wt') as stream:
                for value in values:
                    stream.write(json.dumps(value, separators=(',', ':')) + '\n')
        base_files = {
            'summary.json': {'expected_logs': 1, 'observation': {'epoch_ns': epoch},
                'measurement': {'clock': {'realtime_range_ns': 0, 'suspend_range_ns': 0},
                    'phases': {p: {'samples': 2, 'server_cpu_cores': 1.0, 'reason': None}
                               for p, _ in PHASES}}},
            'seed-summary.json': {'condition': 'H0', 'encoded_bytes': 0, 'batches': 0,
                                  'rows': 0, 'prefix_sha256': hashlib.sha256(b'').hexdigest()},
            'seed-verification.json': {'seed_replay_exact': True, 'seed_batch_hash_mismatches': 0,
                'seed_body_fidelity_in_exact_source_gate': True, 'seed_batches': 0,
                'seed_source_rows': 0, 'prefix_sha256': hashlib.sha256(b'').hexdigest()},
            'query-verdicts.json': {'final': [{'passed': True}],
                'controls': [{'verdict': {'passed': False}}, {'verdict': {'passed': False}}]},
            'query-provenance.json': {'oracle_path': str(ROOT / 'tools/qualification/query_oracle.py'),
                'oracle_sha256': digest(ROOT / 'tools/qualification/query_oracle.py'),
                'recovered_sha256': hashlib.sha256(b'replay').hexdigest(),
                'verdict_file': 'query-verdicts.json', 'quiescent_query_count': 1,
                'negative_control_count': 2},
            'environment.json': {'host': 'negative-control',
                'protocol_sha256': digest(ROOT / 'docs/experiments/benchmarks/dev-small-labs-screen-protocol.md'),
                'sources': {'tools/bench/labs/dev_small/audit.py': digest(Path(__file__).resolve()),
                    'tools/qualification/query_oracle.py': digest(ROOT / 'tools/qualification/query_oracle.py')}},
        }
        def build_cell():
            for path, value in base_files.items():
                put_json(path, value)
            with gzip.open(root / 'seed-ledger.jsonl.gz', 'wt'):
                pass
            put_lines('recovered-hashes.jsonl.gz', [['node00', 1, batch_sha, 9, epoch]])
            put_lines('sources.jsonl.gz', [[tag, body_sha, epoch, 'normal', 0.0]])
            put_lines('data-clocks.jsonl.gz', [[tag, 'node00', 1, epoch, epoch, epoch, epoch, body_sha]])
            events = [{'t': epoch, 'kind': 'cycle', 'batch': 1},
                      {'t': epoch, 'kind': 'ack_attempt', 'sequence': 1, 'sha256': batch_sha,
                       'status': 'ack'}]
            put_lines('node00-events.jsonl.gz', events)
            sample_rows = []
            for seconds in (0, 1, 60, 61, 120, 121):
                wall, mono = epoch + seconds * 10**9, mono0 + seconds * 10**9
                sample_rows.append({'wall_ns': wall, 'mono_ns': mono, 'observer_wall_ns': wall,
                    'observer_mono_ns': mono, 'boot_ns': mono + 10**9,
                    'processes': [{'cpu_s': float(seconds)}]})
            (root / 'resources.json').write_text(json.dumps(sample_rows))
            files = []
            for path in sorted(root.iterdir()):
                if path.name != 'artifact-manifest.json':
                    files.append({'path': path.name, 'sha256': digest(path)})
            put_json('artifact-manifest.json', {'files': files})
        def require_rejected(label, mutate, expected):
            build_cell()
            mutate()
            # Re-sign deliberate mutations so the semantic check, rather than
            # the artifact-hash layer, must reject each control.
            files = [{'path': p.name, 'sha256': digest(p)} for p in sorted(root.iterdir())
                     if p.name != 'artifact-manifest.json']
            put_json('artifact-manifest.json', {'files': files})
            verdict = audit_data(root)
            assert any(expected in issue for issue in verdict['problems']), (label, verdict['problems'])
            checks[label] = True
        assert not audit_data_after_build(build_cell, root)
        # Review counterexample: the checked recovered_sha256 was previously
        # copied from a nonexistent replay_sha256 key into the display report.
        assert audit_data(root)['query_provenance']['replay_sha256'] == hashlib.sha256(b'replay').hexdigest()
        checks['checked_replay_hash_is_reported'] = True
        require_rejected('ack_hash_mismatch_rejected',
            lambda: put_lines('node00-events.jsonl.gz', [{'t': epoch, 'kind': 'cycle', 'batch': 1},
                {'t': epoch, 'kind': 'ack_attempt', 'sequence': 1, 'sha256': hashlib.sha256(b'wrong').hexdigest(), 'status': 'ack'}]),
            'ACK/recovered encoded Batch hash differs')
        require_rejected('source_body_mismatch_rejected',
            lambda: put_lines('data-clocks.jsonl.gz', [[tag, 'node00', 1, epoch, epoch, epoch, epoch,
                hashlib.sha256(b'wrong').hexdigest()]]), 'recovered source body hash differs')
        def alter_clock():
            sample_path = root / 'resources.json'
            sample_rows = json.loads(sample_path.read_text())
            sample_rows[-1]['observer_wall_ns'] += 5_000_001
            sample_path.write_text(json.dumps(sample_rows))
        require_rejected('clock_discontinuity_rejected', alter_clock,
            'reported realtime range differs from raw samples')
        require_rejected('query_verdict_failure_rejected',
            lambda: put_json('query-verdicts.json', {'final': [{'passed': False}],
                'controls': [{'verdict': {'passed': False}}, {'verdict': {'passed': False}}]}),
            'independent final query verdict failed')
    return {'semantic_negative_controls': checks, 'temporary_fixture_removed': not Path(name).exists()}


def audit_data_after_build(build, root):
    build()
    return audit_data(root)['problems']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--controls', action='store_true')
    parser.add_argument('--cell', type=Path)
    args = parser.parse_args()
    require_limits()
    if args.controls:
        report = controls()
        print(json.dumps(report))
        return
    if args.cell is None:
        parser.error('provide --cell PATH or --controls')
    root = args.cell.resolve()
    report = audit_data(root)
    report['cell'] = str(root)
    out = root / 'independent-audit.json'
    if out.exists():
        raise RuntimeError('refusing to overwrite independent-audit.json')
    out.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))
    raise SystemExit(0 if report['passed'] else 1)


if __name__ == '__main__':
    main()
