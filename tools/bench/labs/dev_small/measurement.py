"""Clock, phase, and visibility measurements for the dev-small lab screen.

This module extends the native observer without changing its archived protocol
or the readiness query oracle.  The coordinator owns workload execution.
"""
from __future__ import annotations

import copy
import gzip
import hashlib
import json
import time
from collections import Counter
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools/bench'))
import observe_dev_small as observation

CLOCK_FIXTURE = ROOT / 'docs/experiments/benchmarks/data/readiness-labs-run-01/query/clock-discontinuity-fixture.json'
PHASES = ('normal', 'burst', 'recovery')
GUARD_NS = 5_000_000
LAB_RETENTION_BYTES = 50 * 2**20


def _offsets(samples):
    """Return integer offsets and discontinuity ranges; never round epoch ns."""
    rt = [s.get('observer_wall_ns', s['wall_ns']) -
          s.get('observer_mono_ns', s['mono_ns']) for s in samples]
    bt = [s['boot_ns'] - s.get('observer_mono_ns', s['mono_ns'])
          for s in samples if s.get('boot_ns') is not None]
    return {'realtime_minus_monotonic_ns': rt,
            'boottime_minus_monotonic_ns': bt,
            'realtime_range_ns': max(rt) - min(rt) if rt else None,
            'suspend_range_ns': max(bt) - min(bt) if bt else None,
            'boottime_available': len(bt) == len(samples) and bool(samples)}


def _phase_metrics(samples, epoch_ns):
    """Preserve absent phases as null, with a reason, not fabricated zeroes."""
    rows = {p: [s for s in samples if observation.classify(s['wall_ns'], epoch_ns) == p]
            for p in PHASES}
    out = {}
    for phase, group in rows.items():
        if len(group) < 2:
            out[phase] = {'samples': len(group), 'server_cpu_cores': None,
                          'reason': 'fewer than two resource samples in phase'}
            continue
        elapsed = group[-1]['mono_ns'] - group[0]['mono_ns']
        cpu = group[-1]['processes'][0]['cpu_s'] - group[0]['processes'][0]['cpu_s']
        out[phase] = {'samples': len(group),
                      'server_cpu_cores': cpu / (elapsed / 1e9) if elapsed > 0 else None,
                      'reason': None if elapsed > 0 else 'nonpositive monotonic phase duration'}
    return out


def _assessment(samples, summary, epoch_ns):
    offsets = _offsets(samples)
    phases = _phase_metrics(samples, epoch_ns)
    clock_ok = offsets['realtime_range_ns'] is not None and offsets['realtime_range_ns'] <= GUARD_NS
    suspend_ok = offsets['boottime_available'] and offsets['suspend_range_ns'] <= GUARD_NS
    phase_ok = all(phases[p]['server_cpu_cores'] is not None for p in PHASES)
    custody_ok = bool(summary.get('gates', {}).get('exact_source_logs') and
                      summary.get('gates', {}).get('ack_hash_recovery'))
    rate_ok = summary.get('gates', {}).get('rate_accounting_exact', False)
    reasons = []
    if not clock_ok:
        reasons.append('realtime-minus-monotonic range exceeds 5 ms or has no samples')
    if not suspend_ok:
        reasons.append('boottime-minus-monotonic range exceeds 5 ms or boottime unavailable')
    return {'clock': {'realtime_monotonic_valid': clock_ok,
                      'suspend_interval_valid': suspend_ok,
                      'realtime_range_ns': offsets['realtime_range_ns'],
                      'suspend_range_ns': offsets['suspend_range_ns'],
                      'guard_ns': GUARD_NS,
                      'reason': '; '.join(reasons) if reasons else None},
            'phases': phases,
            'valid_for_timing': clock_ok and suspend_ok and phase_ok and custody_ok and rate_ok,
            'invalid_reasons': reasons + [reason for failed, reason in (
                (not phase_ok, 'one or more required phases lacks two samples'),
                (not custody_ok, 'native exact source/ACK custody gate failed'),
                (not rate_ok, 'rate accounting gate failed')) if failed]}


def _custody_partition(seed_rows, recovered_rows, live_cycles, live_acks,
                       seed_replay_exact):
    """Prove seed and live sets independently cover the recovered batches."""
    seed_set, recovered_set = set(seed_rows), set(recovered_rows)
    seed_ids = {(row[0], row[1]) for row in seed_set}
    live_ids = {(row[0], row[1]) for row in recovered_set if (row[0], row[1]) not in seed_ids}
    seed_exact = (len(seed_rows) == len(seed_set) and seed_set <= recovered_set and
                  seed_replay_exact)
    live_exact = (set(live_cycles) == set(live_acks) == live_ids and
                  len(recovered_set) == len(seed_set) + len(live_ids))
    return {'passed': seed_exact and live_exact,
            'seed_exact': seed_exact, 'live_exact': live_exact,
            'seed_rows': len(seed_rows), 'seed_distinct_rows': len(seed_set),
            'live_recovered_batches': len(live_ids),
            'combined_recovered_batches': len(recovered_set)}


def _rate_counts_match(expected, reported):
    return {key: int(value) for key, value in expected.items() if value} == {
        key: int(value) for key, value in reported.items() if value}


def _digest(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


class Observer(observation.Observer):
    """Native observer with raw clock guards and optional deterministic jitter.

    ``cell='off'`` retains QueryOff sampling and post-run oracle behavior.
    ``visibility_jitter`` selects source sentinels by a reproducible offset
    sequence, changing their phase relative to collection without moving time.
    """
    def __init__(self, cell='scan', visibility_jitter=False, jitter_ns=None,
                 native_module=None, target_selector=None):
        super().__init__()
        if cell not in ('off', 'scan', 'walk', 'q1-fresh', 'q1-seeded'):
            raise ValueError("unsupported query measurement cell")
        self.cell = cell
        self.native = native_module or observation.native
        self.target_selector = target_selector or self.select_visibility_target
        self.visibility_jitter = bool(visibility_jitter)
        self.jitter_ns = tuple(jitter_ns or (137_000_000, 311_000_000, 73_000_000,
                                             419_000_000, 191_000_000))
        if not self.jitter_ns or any(type(v) is not int or v < 0 for v in self.jitter_ns):
            raise ValueError('jitter_ns must be a nonempty sequence of nonnegative integers')
        self._jitter_origin_ns = None
        self._jitter_ordinal = 0
        self._last_target_bucket = -1
        self.seed_summary = None
        self.seed_ledger = []

    def sample(self, root, kids, events):
        wall_ns = time.time_ns()
        mono_ns = time.monotonic_ns()
        boot_ns = (time.clock_gettime_ns(time.CLOCK_BOOTTIME)
                   if hasattr(time, 'CLOCK_BOOTTIME') else None)
        row = super().sample(root, kids, events)
        row.update(observer_wall_ns=wall_ns, observer_mono_ns=mono_ns,
                   boot_ns=boot_ns)
        return row

    def start(self, api, epoch, nodes):
        if self.cell == 'off':
            self.api, self.epoch = api, epoch
            return
        super().start(api, epoch, nodes)

    def target(self, tag, sha, source_ns, phase):
        if self.cell != 'off':
            super().target(tag, sha, source_ns, phase)

    def seed_info(self, root, seed_summary):
        """Capture the private native harness's independently verified seed."""
        self.seed_summary = copy.deepcopy(seed_summary)
        ledger = Path(root) / 'seed-ledger.jsonl'
        with ledger.open() as stream:
            self.seed_ledger = [json.loads(line) for line in stream if line.strip()]
        if not self.seed_ledger and any(int(seed_summary.get(k, 0)) for k in
                                        ('rows', 'batches', 'encoded_bytes')):
            raise RuntimeError('nonempty seed summary supplied without a custody ledger')

    def quiescent(self, api):
        super().quiescent(api)
        if not self.seed_ledger:
            return
        sentinels = []
        for entry, index in ((self.seed_ledger[0], 0), (self.seed_ledger[-1], -1)):
            source = entry.get('source') or []
            if not source:
                raise RuntimeError('seed ledger entry has no sentinel source rows')
            sentinels.append(source[index][0])
        for label in dict.fromkeys(sentinels):
            query = dict(observation.QUERY, contains=label, limit=7)
            pages, current = [], query
            for _ in range(100):
                page = api('/v1/admin/query', current)
                pages.append(page)
                if page['next_page'] is None:
                    break
                current = dict(query, page=page['next_page'])
            else:
                raise RuntimeError('seed sentinel pagination exceeded 100 pages')
            self.finals.append({'query': query, 'pages': pages,
                                'seed_sentinel': label})

    def select_visibility_target(self, *args, **kwargs):
        """Native harness hook: select source writes at jittered 5s targets.

        Call for each eligible source write with its integer ``source_ns``.
        This returns a selection decision; it never changes event timestamps.
        """
        candidate = args[0] if args else kwargs.get('candidate')
        tick = args[1] if len(args) > 1 else kwargs.get('tick')
        if isinstance(candidate, dict):
            candidate = candidate.get('source_ns')
        if type(candidate) is not int:
            raise TypeError('visibility candidate must provide integer source_ns')
        if self.cell == 'off':
            return False
        if not self.visibility_jitter:
            if not isinstance(tick, int) or tick < 0:
                return False
            bucket = tick // 50
            if bucket <= self._last_target_bucket:
                return False
            self._last_target_bucket = bucket
            return True
        if self._jitter_origin_ns is None:
            self._jitter_origin_ns = candidate
            self._jitter_ordinal = 0
        due = (self._jitter_origin_ns +
               self._jitter_ordinal * 5_000_000_000 +
               self.jitter_ns[self._jitter_ordinal % len(self.jitter_ns)])
        if candidate < due:
            return False
        self._jitter_ordinal += 1
        return True

    def finish(self, root, summary, samples, events):
        if self.cell == 'off':
            self._finish_off(root, summary, samples, events)
        else:
            super().finish(root, summary, samples, events)
        if self.seed_ledger:
            self._grade_seed_partition(root, summary, events)
        epoch = summary.get('observation', {}).get('epoch_ns', getattr(self, 'epoch', 0))
        rate_check = self._check_rates(root, summary, events, epoch)
        summary.setdefault('gates', {})['rate_accounting_exact'] = rate_check['passed']
        summary.setdefault('measurement', {})['rate_accounting'] = rate_check
        summary.setdefault('measurement', {}).update(_assessment(samples, summary, epoch))
        summary['measurement']['cell'] = self.cell
        summary['measurement']['visibility_jitter'] = {
            'enabled': self.visibility_jitter,
            'schedule_ns': list(self.jitter_ns) if self.visibility_jitter else [],
            'policy': 'repeats cyclically by target arrival order' if self.visibility_jitter else 'disabled'}
        summary['passed'] = all(summary['gates'].values())
        self.native.dump(root/'summary.json', summary)
        if self.seed_summary is not None:
            self.native.dump(root/'seed-observation.json', self.seed_summary)
        self._write_query_provenance(root)

    def _grade_seed_partition(self, root, summary, events):
        recovered = []
        with gzip.open(root/'recovered-hashes.jsonl.gz', 'rt') as stream:
            for line in stream:
                label, sequence, sha, size, _received = json.loads(line)
                recovered.append((label, int(sequence), sha, int(size)))
        recovered_set = set(recovered)
        seed = [(row['label'], int(row['sequence']), row['sha256'], int(row['bytes']))
                for row in self.seed_ledger]
        seed_set = set(seed)
        live_cycles = {(node, int(row['batch'])) for node, rows in events.items()
                       for row in rows if row['kind'] == 'cycle'}
        live_acks = {(node, int(row['sequence'])) for node, rows in events.items()
                     for row in rows if row['kind'] == 'ack_attempt' and row['status'] == 'ack'}
        verification_path = root/'seed-verification.json'
        verification = (json.loads(verification_path.read_text())
                        if verification_path.exists() else self.seed_summary or {})
        seed_verification = bool(verification.get('seed_replay_exact') is True and
            verification.get('seed_batch_hash_mismatches') == 0)
        partition = _custody_partition(seed, recovered, live_cycles, live_acks,
                                       seed_verification)
        prior = summary.get('gates', {}).get('spool_cycle_custody')
        summary.setdefault('measurement', {})['custody_partition'] = {
            'live_cycle_ack_recovered_exact': partition['live_exact'],
            'seed_ledger_recovered_exact': partition['seed_exact'],
            'seed_replay_oracle_exact': seed_verification,
            'seed_rows': partition['seed_rows'],
            'seed_distinct_rows': partition['seed_distinct_rows'],
            'live_recovered_batches': partition['live_recovered_batches'],
            'combined_recovered_batches': partition['combined_recovered_batches'],
            'inherited_combined_gate': prior,
            'method': 'partition recovered batch identities into seed ledger and live cycle/ACK observations'}
        summary['gates']['spool_cycle_custody'] = partition['passed']
        summary['passed'] = all(summary['gates'].values())

    def _check_rates(self, root, summary, events, epoch):
        sizes = {}
        with gzip.open(root/'recovered-hashes.jsonl.gz', 'rt') as stream:
            for line in stream:
                label, seq, _sha, size, _received = json.loads(line)
                sizes[(label, int(seq))] = int(size)
        expected = {phase: Counter() for phase in PHASES}
        seen_acks = set()
        for node, rows in events.items():
            for event in rows:
                phase = observation.classify(int(event['t']), epoch)
                if phase not in expected:
                    continue
                counts = expected[phase]
                if event['kind'] == 'cycle':
                    key = (node, int(event['batch']))
                    if key not in sizes:
                        return {'passed': False, 'reason': 'cycle missing recovered batch size'}
                    counts.update(spool_batches=1, spool_logs=int(event['logs']),
                                  spool_bytes=sizes[key])
                else:
                    counts.update(send_attempts=1, **{'status_'+event['status']: 1})
                    if event['status'] == 'ack':
                        key = (node, int(event['sequence']))
                        if key not in seen_acks:
                            seen_acks.add(key)
                            counts.update(unique_acks=1, ack_bytes=sizes.get(key, -10**18))
        with gzip.open(root/'sources.jsonl.gz', 'rt') as stream:
            for line in stream:
                source = json.loads(line)
                phase = observation.classify(int(source[2]), epoch)
                if phase in expected:
                    expected[phase].update(source_logs=1, source_bytes=901)
        actual = summary.get('observation', {}).get('rates', {})
        matches = {phase: _rate_counts_match(expected[phase],
                    actual.get(phase, {}).get('counts', {})) for phase in PHASES}
        return {'passed': all(matches.values()), 'phase_counts_exact': matches,
                'expected_counts': {p: dict(c) for p, c in expected.items()},
                'method': 'recount event and source rows against independently retained phase counts'}

    def _write_query_provenance(self, root):
        oracle_path = Path(observation.native.query_oracle.__file__).resolve()
        measurement_path = Path(__file__).resolve()
        recovered_path = root/'recovered.jsonl'
        verdict_path = root/'query-verdicts.json'
        verdicts = json.loads(verdict_path.read_text()) if verdict_path.exists() else {}
        self.native.dump(root/'query-provenance.json', {
            'measurement_path': str(measurement_path),
            'measurement_sha256': _digest(measurement_path),
            'oracle_path': str(oracle_path),
            'oracle_sha256': _digest(oracle_path),
            'recovered_sha256': _digest(recovered_path)
                if recovered_path.exists() else None,
            'verdict_file': verdict_path.name if verdict_path.exists() else None,
            'quiescent_query_count': len(verdicts.get('final', [])),
            'negative_control_count': len(verdicts.get('controls', [])),
            'seeded_sentinel_count': sum('seed_sentinel' in q for q in self.finals),
            'note': 'The archived final verdicts and missing/changed controls are from the unchanged Python oracle.'})

    def _finish_off(self, root, summary, samples, events):
        # Match the established QueryOff lifecycle: run the unchanged verifier,
        # then explicitly mark only timed query and visibility dimensions N/A.
        super().finish(root, summary, samples, events)
        from labs.readiness.query.run import QueryOff
        QueryOff.mark_unmeasured(summary)
        self.native.dump(root/'summary.json', summary)
        self._write_query_provenance(root)


def controls():
    """Deterministic defects the measurement gate must reject."""
    # Existing native and QueryOff controls preserve their independent checks.
    from labs.readiness.query.run import controls as readiness_controls
    checks = readiness_controls()
    base = {'gates': {'exact_source_logs': True, 'ack_hash_recovery': True,
                      'rate_accounting_exact': True}}
    epoch = 10_000_000_000
    offsets = (0, 1_000_000_000, 60_000_000_000, 61_000_000_000,
               120_000_000_000, 121_000_000_000)
    samples = [{'wall_ns': epoch + offset,
                'mono_ns': 9_000_000_000 + offset,
                'observer_wall_ns': epoch + offset,
                'observer_mono_ns': 9_000_000_000 + offset,
                'boot_ns': 11_000_000_000 + offset,
                'processes': [{'cpu_s': float(i)}]} for i, offset in enumerate(offsets)]
    assert _assessment(samples, base, epoch)['valid_for_timing']
    discontinuity = copy.deepcopy(samples)
    discontinuity[-1]['wall_ns'] += GUARD_NS + 1
    discontinuity[-1]['observer_wall_ns'] += GUARD_NS + 1
    assert not _assessment(discontinuity, base, epoch)['valid_for_timing']
    suspend = copy.deepcopy(samples)
    suspend[-1]['boot_ns'] += GUARD_NS + 1
    assert not _assessment(suspend, base, epoch)['valid_for_timing']
    assert not _assessment(samples[:2], base, epoch)['valid_for_timing']
    bad_rate = {'gates': dict(base['gates'], rate_accounting_exact=False)}
    bad_custody = {'gates': dict(base['gates'], ack_hash_recovery=False)}
    assert not _assessment(samples, bad_rate, epoch)['valid_for_timing']
    assert not _assessment(samples, bad_custody, epoch)['valid_for_timing']
    assert _rate_counts_match({'source_logs': 1, 'source_bytes': 901},
                              {'source_logs': 1, 'source_bytes': 901})
    assert not _rate_counts_match({'source_logs': 1, 'source_bytes': 901},
                                  {'source_logs': 2, 'source_bytes': 1802})
    seed_row = ('old-node', 1, 'seed-hash', 901)
    live_row = ('node00', 2, 'live-hash', 901)
    partition = _custody_partition([seed_row], [seed_row, live_row],
                                   {('node00', 2)}, {('node00', 2)}, True)
    assert partition['passed']
    assert not _custody_partition([('old-node', 1, 'wrong-hash', 901)],
        [seed_row, live_row], {('node00', 2)}, {('node00', 2)}, True)['passed']
    assert not _custody_partition([seed_row], [seed_row, live_row],
        {('node00', 2)}, set(), True)['passed']
    with CLOCK_FIXTURE.open() as stream:
        archived = json.load(stream)
    assert archived['passed'] is False and not archived['gates']['clock_offset_range_le_5ms']
    assert archived['observation']['cpu']['recovery']['samples'] == 0
    assert archived['clock_offset_range_ms'] > 5
    # Check fresh-development observer configuration only. An actual native
    # preflight requires the coordinator's bounded M0 trial.
    fresh = Observer(cell='q1-fresh', visibility_jitter=True)
    assert fresh.cell == 'q1-fresh' and fresh.visibility_jitter
    assert set(PHASES) == set(_phase_metrics(samples, epoch))
    selected = []
    default = Observer(cell='scan')
    off = Observer(cell='off', visibility_jitter=True)
    for tick in range(1800):
        candidate_ns = tick * 100_000_000
        if fresh.select_visibility_target(candidate_ns):
            selected.append(candidate_ns)
    assert len(selected) == 36
    assert any((t % 5_000_000_000) != 0 for t in selected)
    assert sum(default.select_visibility_target(tick*100_000_000, tick)
               for tick in range(1800)) == 36
    assert sum(off.select_visibility_target(tick*100_000_000, tick)
               for tick in range(1800)) == 0
    # Review counterexample: fractional per-node offers have no record on
    # normal/recovery bucket boundaries. Exercise the actual rate accumulator.
    fixture = json.loads(Path(__file__).with_name('fractional-selector-fixture.json').read_text())
    fractional = Observer(cell='scan')
    eligible, repaired = [], []
    accumulator = 0
    for tick in range(fixture['ticks']):
        accumulator += fixture['rates'][tick // 600]
        count, accumulator = divmod(accumulator, 10 * fixture['nodes'])
        if count:
            eligible.append(tick)
            if fractional.select_visibility_target(tick * 100_000_000, tick):
                repaired.append(tick)
    old = [tick for tick in eligible if tick % 50 == 0]
    assert len(old) == fixture['old_target_count'] == 12
    assert len(repaired) == fixture['required_target_count'] == 36
    assert [tick // 50 for tick in repaired] == list(range(36))
    assert repaired[:12] == list(range(1, 600, 50))
    assert repaired[12:24] == list(range(600, 1200, 50))
    assert repaired[24:] == list(range(1201, 1800, 50))
    checks.update(archived_clock_fixture_rejected=True,
                  missing_phase_null_and_timing_invalid=True,
                  injected_clock_suspend_rate_custody_defects_rejected=True,
                  fresh_development_configuration_constructible=True,
                  default_selector_36_targets=True,
                  fractional_selector_counterexample={'origin': fixture['origin'],
                      'old_selected_ticks': old, 'repaired_selected_ticks': repaired},
                  jitter_selector_36_targets_not_phase_locked=True,
                  query_off_selector_no_targets=True)
    return checks


__all__ = ['Observer', 'controls']
