#!/usr/bin/env python3
"""Recompute frozen screen accounting and classify a nomination, never confirmation."""
import argparse
import copy
import gzip
import hashlib
import json
from pathlib import Path

import run as lab

PHASES = ('normal', 'burst', 'recovery')


def require_equal(actual, expected, reason):
    if actual != expected:
        raise AssertionError(reason)


def load(path):
    opener = gzip.open if path.suffix == '.gz' else open
    with opener(path, 'rt') as stream:
        return json.load(stream)


def fidelity(summary, verdict):
    return (summary['passed'] and all(summary['gates'].values()) and
            all(v['passed'] for v in verdict['final']) and
            all(not c['verdict']['passed'] for c in verdict['controls']) and
            verdict['changed_live_rows'] == 0 and verdict['unmatched_live_rows'] == 0)


def guards(scan, walk):
    so, wo = scan['observation'], walk['observation']
    sm, wm = scan['lab']['metrics'], walk['lab']['metrics']
    rss = wm['offered_phase_peak_server_rss_mib'] / sm['offered_phase_peak_server_rss_mib'] - 1
    backlog = all(wo[k] <= so[k] for k in ('peak_pending_batches', 'peak_file_backlog_bytes'))
    backlog &= all(sum(p[k] for p in wo['progress_final']) <= sum(p[k] for p in so['progress_final'])
                   for k in ('pending_batches', 'file_backlog_bytes'))
    backlog &= walk['non_ack_attempts'] <= scan['non_ack_attempts']
    backlog &= walk['latency_ms']['all']['ack']['p99'] <= scan['latency_ms']['all']['ack']['p99']
    cpu = 1 - wm['balanced_phase_server_cores'] / sm['balanced_phase_server_cores']
    a, b = sm['balanced_phase_shape_median_ms'], wm['balanced_phase_shape_median_ms']
    latency = 1 - b / a if a is not None and b is not None and a > 0 else None
    return {'cpu_improvement_fraction': cpu, 'median_latency_improvement_fraction': latency,
            'rss_growth_fraction': rss, 'rss_guard': rss <= .10, 'backlog_guard': bool(backlog),
            'practical_gain': cpu >= .15 or (latency is not None and latency >= .15)}


def inspect(cell):
    summary, env = load(cell / 'summary.json'), load(cell / 'environment.json')
    samples = load(cell / 'resources.json.gz' if (cell / 'resources.json.gz').exists() else cell / 'resources.json')
    verdict = load(cell / 'query-verdicts.json')
    batch_sizes = {}
    with gzip.open(cell / 'recovered-hashes.jsonl.gz', 'rt') as stream:
        for line in stream:
            node, seq, sha, size, received = json.loads(line)
            batch_sizes[(node, seq)] = size
    events = {}
    for path in cell.glob('node*-events.jsonl.gz'):
        with gzip.open(path, 'rt') as stream:
            events[path.name.split('-events')[0]] = [json.loads(line) for line in stream]
    epoch = summary['observation']['epoch_ns']
    rates = lab.observation.phase_rates(epoch, max(s['wall_ns'] for s in samples), batch_sizes, events, cell / 'sources.jsonl.gz')
    require_equal(rates, summary['observation']['rates'], 'raw rate recomputation mismatch')
    assert lab.observation.cpu_windows(samples, epoch) == summary['observation']['cpu'], 'raw CPU recomputation mismatch'
    source_hash = hashlib.sha256()
    phases = dict(normal=0, burst=0, recovery=0)
    with gzip.open(cell / 'sources.jsonl.gz', 'rt') as stream:
        for line in stream:
            tag, sha, source, phase, lag = json.loads(line)
            source_hash.update(json.dumps([tag, sha, phase], separators=(',', ':')).encode() + b'\n')
            phases[phase] += 1
    assert phases == dict(normal=60000, burst=180000, recovery=60000), 'offered phase fixture mismatch'
    assert summary['expected_logs'] == summary['recovered_logs'] == 300000, 'source conservation mismatch'
    assert summary['nodes'] == 20 and summary['deployment'] == 'small'
    assert all(rates[p]['window_seconds'] == 60 for p in PHASES)
    assert fidelity(summary, verdict), 'custody/query fidelity failure'
    cleanup = load(cell / 'cleanup.json')
    assert cleanup['status'] == 'passed' and cleanup['removed'], 'cleanup incomplete'
    if summary['lab']['cell'] == 'off':
        assert summary['observation']['queries']['status'] == 'not_applicable'
        assert summary['observation']['visibility']['value'] is None
        assert summary['lab']['metrics']['balanced_phase_shape_median_ms'] is None
        with gzip.open(cell / 'queries.jsonl.gz', 'rt') as stream:
            assert not stream.read(), 'off cell performed timed queries'
    return summary, env, source_hash.hexdigest()


def controls():
    summary = {'passed': True, 'gates': {'exact_source_logs': True}}
    verdict = {'final': [{'passed': True}], 'controls': [{'verdict': {'passed': False}}],
               'changed_live_rows': 0, 'unmatched_live_rows': 0}
    assert fidelity(summary, verdict)
    changed = copy.deepcopy(verdict)
    changed['final'][0]['passed'] = False
    assert not fidelity(summary, changed), 'changed oracle answer escaped'
    row = {'lab': {'metrics': {'balanced_phase_server_cores': 1,
         'balanced_phase_shape_median_ms': 100, 'offered_phase_peak_server_rss_mib': 100}},
         'observation': {'peak_pending_batches': 2, 'peak_file_backlog_bytes': 10,
            'progress_final': [{'pending_batches': 0, 'file_backlog_bytes': 0}]},
         'non_ack_attempts': 0, 'latency_ms': {'all': {'ack': {'p99': 10}}}}
    better = copy.deepcopy(row)
    better['lab']['metrics']['balanced_phase_server_cores'] = .8
    assert guards(row, better)['practical_gain']
    better['lab']['metrics']['offered_phase_peak_server_rss_mib'] = 111
    assert not guards(row, better)['rss_guard'], 'RSS defect escaped'
    raw, changed_rate = {'source_logs': 1000}, {'source_logs': 999}
    rejected = False
    try:
        require_equal(raw, changed_rate, 'injected changed rate')
    except AssertionError:
        rejected = True
    assert rejected, 'changed rate escaped exact recomputation comparator'
    off = copy.deepcopy(row)
    off['lab']['metrics']['balanced_phase_shape_median_ms'] = None
    assert guards(row, off)['median_latency_improvement_fraction'] is None
    return {'changed_answer_rejected': True, 'rss_regression_rejected': True,
            'changed_rate_rejected': True, 'missing_latency_unmeasured': True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--controls', action='store_true')
    parser.add_argument('--data', type=Path, default=lab.PROTOCOL.parent)
    parser.add_argument('--out', type=Path)
    args = parser.parse_args()
    lab.observation.require_limits()
    checks = controls()
    if args.controls:
        print(json.dumps(checks))
        return
    cells = {c: inspect(args.data / c) for c in ('off', 'scan', 'walk')}
    baseline = cells['scan'][1]
    for c, (summary, env, fixture) in cells.items():
        assert fixture == cells['scan'][2], 'source workload fixture drift'
        for key in ('binaries', 'protocol_sha256', 'native_sha256', 'observer_sha256',
                    'harness_sha256', 'server_cpus', 'client_cpus', 'uname', 'limits', 'build_settings'):
            assert env[key] == baseline[key], f'cross-cell {key} drift'
        assert summary['lab']['cell'] == c
        assert summary['lab']['query_plan'] == ('walk' if c == 'walk' else 'scan')
    scan, walk, off = (cells[c][0] for c in ('scan', 'walk', 'off'))
    result = guards(scan, walk)
    decision = 'H1_screen_nomination' if all(result[k] for k in ('rss_guard', 'backlog_guard', 'practical_gain')) else 'H0_screen_not_nominated'
    result.update(decision=decision, optimization_confirmed=False, controls=checks,
        query_off_cpu_attribution_fraction=1 - off['lab']['metrics']['balanced_phase_server_cores'] / scan['lab']['metrics']['balanced_phase_server_cores'],
        cells={c: {'metrics': v[0]['lab']['metrics'], 'phase_cpu': v[0]['observation']['cpu'],
                   'phase_rates': v[0]['observation']['rates'], 'latency': v[0]['observation']['queries']} for c, v in cells.items()})
    out = args.out or args.data / 'comparison.json'
    if out.exists():
        raise RuntimeError('comparison already exists; refusing overwrite')
    if not out.resolve().is_relative_to(lab.PROTOCOL.parent.resolve()):
        raise RuntimeError('comparison output must stay in owned evidence directory')
    lab.native.dump(out, result)
    print(json.dumps({'decision': decision, 'guards': {k: result[k] for k in ('rss_guard', 'backlog_guard', 'practical_gain')}}))


if __name__ == '__main__':
    main()
