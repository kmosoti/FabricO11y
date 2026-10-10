"""Finite native cadence/observer sweep; root coordinator builds and launches it.

Derived custody/query grading and archival helpers come from prefix_load.py.
No source, oracle, protocol, build or production configuration is changed here.
"""
import argparse
import copy
import gzip
import json
import math
import os
from pathlib import Path
import shutil
import sys
import time

import prefix_load as prior

ROOT = prior.ROOT
MIB = 1024 ** 2
SUCCESS_CAP = 100 * MIB
FAILURE_CAP = 128 * MIB
SEEDS = (2703204361, 2703204362)


def quantiles(rows):
    values = sorted(r['latency_ns'] for r in rows)
    if not values:
        return {'count': 0, 'p50_ns': None, 'p95_ns': None, 'p99_ns': None, 'max_ns': None}
    return {'count': len(values), **{name: values[math.ceil(len(values) * q) - 1]
            for name, q in [('p50_ns', .50), ('p95_ns', .95), ('p99_ns', .99)]},
            'max_ns': values[-1]}


def validate_telemetry(result, acks, hooks, timeline):
    if not result['complete'] or not result['exact_batch_custody'] or not result['restart_retry']:
        raise RuntimeError('native custody/restart assertions unavailable')
    if len(acks) != 512 or result['offered'] != 512 or result['accepted'] != 512:
        raise RuntimeError('incomplete offered/accepted population')
    for i, row in enumerate(acks):
        sequence = 4 + i // 128
        if row['sequence'] != sequence or row['answer'] != f'Ack({sequence})' or row['bytes'] != 8192:
            raise RuntimeError('ACK sequence/size mismatch')
        if not 0 <= row['offered_ns'] <= row['reply_ns'] <= result['schedule_end_ns']:
            raise RuntimeError('invalid ACK timing')
        if row['latency_ns'] != row['reply_ns'] - row['offered_ns']:
            raise RuntimeError('ACK timing arithmetic mismatch')
    builds = [r for r in hooks if r['name'] == 'bounded_segment_build']
    anchors = [r for r in hooks if r['name'] == 'cadence_schedule']
    if result['observer'] == 'quiet':
        if hooks or timeline:
            raise RuntimeError('quiet observer performed event/poll collection')
    elif len(builds) != 3 or len(anchors) != 1:
        raise RuntimeError('event ledger lacks three builders and one schedule anchor')
    if (result['observer'] == 'polled') != bool(timeline):
        raise RuntimeError('polling arm mismatch')
    if any(r['wall_ns'] < 0 or r['start_ns'] < 0 for r in hooks):
        raise RuntimeError('negative event clock')


def telemetry_controls(result, acks, hooks, timeline):
    rejected = []
    bad = copy.deepcopy(acks)
    bad[0]['answer'] = 'Ack(999)'
    try:
        validate_telemetry(result, bad, hooks, timeline)
    except RuntimeError:
        rejected.append('wrong-ack-sequence')
    if result['observer'] != 'quiet':
        bad_hooks = [r for r in hooks if r['name'] != 'bounded_segment_build']
        try:
            validate_telemetry(result, acks, bad_hooks, timeline)
        except RuntimeError:
            rejected.append('missing-builder-events')
    else:
        try:
            validate_telemetry(result, acks, [{'name': 'injected-observer'}], timeline)
        except RuntimeError:
            rejected.append('quiet-observer-contamination')
    if len(rejected) != 2:
        raise RuntimeError('telemetry checker accepted representative defect')
    return rejected


def report_metrics(case):
    read = lambda name: json.loads((case / name).read_text())
    result, acks, hooks, timeline = map(read, ['result.json', 'acks.json', 'hooks.json', 'timeline.json'])
    validate_telemetry(result, acks, hooks, timeline)
    controls = telemetry_controls(result, acks, hooks, timeline)
    builds = [r for r in hooks if r['name'] == 'bounded_segment_build']
    builder_events = []
    lower_release = 0
    if builds:
        anchor = next(r for r in hooks if r['name'] == 'cadence_schedule')['start_ns']
        lo, hi = result['hook_anchor_schedule_ns_bounds']
        for r in sorted(builds, key=lambda r: r['start_ns']):
            end = r['start_ns'] + r['wall_ns'] - anchor
            builder_events.append({'thread': r['thread'], 'wall_ns': r['wall_ns'],
                'completion_schedule_ns_bounds': [end + lo, end + hi],
                'label': 'unavailable: existing hook has no journal label'})
        # The fixture has no pre-existing Segment, and every reclaim follows a
        # successful builder join. The earliest builder return bounds all three
        # releases from below; it does not identify which builder owns a label.
        lower_release = min(r['completion_schedule_ns_bounds'][0] for r in builder_events)
    debt_bounds = [sum(size * bound for _, size in result['initial_sizes'])
                   for bound in [lower_release, result['sealer_end_ns']]]
    releases = None
    if timeline:
        # Reuse the filename interval method, not prefix_load's Ack(4)-only
        # metric population: this fixture also accepts sequences5,6,7.
        releases = []
        for label, size in result['initial_sizes']:
            last_present, first_absent, publication, previous_begin = 0, None, None, 0
            for event in timeline:
                if label in [pair[0] for pair in event['journals']]:
                    last_present = event['ns_begin']
                elif first_absent is None:
                    first_absent = event['ns']
                if label in event['segments'] and publication is None:
                    publication = [previous_begin, event['ns']]
                previous_begin = event['ns_begin']
            if first_absent is None or publication is None:
                raise RuntimeError('publication/deletion transition not observed')
            releases.append({'label': label, 'journal_bytes': size,
                             'release_ns': [last_present, first_absent],
                             'publication_ns': publication})
        debt_bounds = [sum(row['journal_bytes'] * row['release_ns'][i] for row in releases)
                       for i in (0, 1)]
    ticks = sum(result['process_after'][key] - result['process_before'][key]
                for key in ('utime_ticks', 'stime_ticks'))
    rss = [int(line.split()[1]) * 1024 for event in timeline
           for line in event['process']['rss_status'] if line.startswith('VmRSS:')]
    # Off arms have only endpoint/HWM observations. Do not label endpoints as a
    # continuously sampled schedule peak or compare unlike peak definitions.
    facts = lambda stage, prefix: next(int(line.split()[1]) * 1024
        for line in result[stage]['rss_status'] if line.startswith(prefix))
    last_offer = max(r['offered_ns'] for r in acks)
    last_ack = max(r['reply_ns'] for r in acks)
    during = [r for r in acks if result['sealer_begin_ns'] <= r['offered_ns'] < result['sealer_end_ns']]
    after = [r for r in acks if r['offered_ns'] >= result['sealer_end_ns']]
    offer_ticks = [r['offered_ns'] for r in acks]
    in_flight = []
    pending = high = 0
    for ns, delta in sorted([(r['offered_ns'], 1) for r in acks] + [(r['reply_ns'], -1) for r in acks],
                            key=lambda item: (item[0], -item[1])):
        pending += delta
        high = max(high, pending)
        in_flight.append((ns, pending))
    summary = {**result, 'ack_all': quantiles(acks), 'ack_offered_during_sealing': quantiles(during),
        'ack_offered_after_sealing': quantiles(after), 'unavailable': 0,
        'accepted_bytes': sum(r['bytes'] for r in acks), 'cpu_ticks': ticks,
        'clk_tck': os.sysconf('SC_CLK_TCK'), 'process_cpu_ns': ticks * 1_000_000_000 // os.sysconf('SC_CLK_TCK'),
        'finite_offered_per_s_from_schedule_start': 512 * 1e9 / max(1, last_offer),
        'finite_accepted_per_s_through_last_ack': 512 * 1e9 / max(1, last_ack),
        'first_offer_ns': min(offer_ticks), 'last_offer_ns': last_offer, 'last_ack_ns': last_ack,
        'max_offer_lateness_ns': max(r['offered_ns'] - r['target_ns'] for r in acks),
        'observer_before_rss_bytes': facts('observer_before', 'VmRSS:'),
        'schedule_before_rss_bytes': facts('process_before', 'VmRSS:'),
        'schedule_after_rss_bytes': facts('process_after', 'VmRSS:'),
        'whole_process_hwm_includes_startup_bytes': facts('process_after', 'VmHWM:'),
        'sampled_schedule_rss_peak_bytes': max(rss) if rss else None,
        'initial_backlog_byte_ns_bounds': debt_bounds, 'polled_release_bounds': releases,
        'builder_events': builder_events, 'hook_records': len(hooks),
        'hook_sample_fields': 'not measured: constant zero callback; ledger/timestamp costs remain',
        'max_observed_unanswered_offers': high, 'max_observed_unanswered_payload_bytes': high * 8192,
        'backlog_scope': 'three initial sealed files; active arrivals excluded from journal byte-time',
        'in_flight_scope': 'offer-to-reply observation; not internal queue occupancy',
        'observer_clock_limits': 'builder return is not rename time; no labeled checkpoint span',
        'scope': 'finite native Store/Intake plus one sealer pass; no HTTP/TLS or steady-state qualification',
        'rejected_telemetry_controls': controls}
    return summary, {'result': result, 'acks': acks, 'hooks': hooks, 'timeline': timeline,
                     'observed_unanswered_offers': in_flight}


def file_manifest(case):
    prior.footprint(case)  # Reject symlinks/special entries before hashing.
    return {str(path.relative_to(case)): {'bytes': path.stat().st_size, 'sha256': prior.sha(path)}
            for path in sorted(case.rglob('*')) if path.is_file()}


def main():
    if os.environ.get('FABRIC_CROSS_SYSTEM_COORDINATED') != '1':
        raise RuntimeError('requires inherited root coordinator lock ownership')
    prior.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--protocol', type=Path, required=True)
    parser.add_argument('--proposal', type=Path, required=True)
    args = parser.parse_args()
    for path in (args.binary, args.protocol, args.proposal):
        if not path.is_file() or path.is_symlink():
            raise RuntimeError('frozen binary/registered protocol/proposal missing')
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True)
    work = scratch / out.name
    work.mkdir()
    started = time.monotonic()
    deadline = started + 900
    receipt = {'state': 'running', 'started_unix_ns': time.time_ns(), 'scratch': str(work),
        'binary_sha256': prior.sha(args.binary), 'cases': [], 'before': prior.cgroup(),
        'job_deadline_s': 900, 'case_deadline_s': 20, 'success_evidence_cap_bytes': SUCCESS_CAP,
        'whole_failure_raw_cap_bytes': FAILURE_CAP, 'failure_archive_cap_bytes': FAILURE_CAP,
        'success_retention': 'verified summaries, exact telemetry, full file hashes; one representative full fixture'}
    sources = [Path(__file__).resolve(), ROOT / 'crates/fabric-server/examples/cadence_load_probe.rs',
        Path(prior.__file__).resolve(), ROOT / 'tools/bench/labs/catalog/native_lifecycle_run.py',
        ROOT / 'crates/fabric-server/src/sealer.rs', ROOT / 'crates/fabric-server/src/store.rs',
        ROOT / 'crates/fabric-server/src/segment.rs', ROOT / 'crates/fabric-server/src/segment/bounded.rs',
        ROOT / 'crates/fabric-frame/src/probe.rs', ROOT / 'crates/fabric-frame/src/frame.rs',
        ROOT / 'tools/qualification/query_oracle.py', args.protocol, args.proposal]
    frozen = {str(path): prior.sha(path) for path in sources}
    receipt['source_sha256'] = frozen
    for i, path in enumerate(sources):
        payload = path.read_bytes()
        dest = out / f'source-{i:02}.gz'
        dest.write_bytes(gzip.compress(payload, mtime=0))
        if gzip.decompress(dest.read_bytes()) != payload:
            raise RuntimeError('source snapshot readback mismatch')
    prior.dump(out / 'receipt.json', receipt)
    try:
        receipt['rejected_archive_controls'] = prior.archive.archive_controls(work)
        for shape in ('balanced3', 'skewed3', 'worker1'):
            for cadence in (0, 2, 5):
                for seed in SEEDS:
                    observers = ('quiet', 'events', 'polled') if seed % 2 else ('polled', 'events', 'quiet')
                    for observer in observers:
                        if time.monotonic() >= deadline:
                            raise TimeoutError('finite operations sweep deadline')
                        if prior.footprint(out) > SUCCESS_CAP - 24 * MIB:
                            raise RuntimeError('100MiB success reserve exhausted')
                        label = f'{shape}-{cadence}ms-{seed}-{observer}'
                        evidence = out / label
                        evidence.mkdir()
                        case = work / label
                        commands = []
                        receipt['active_case'] = {'label': label, 'scratch': str(case), 'commands': commands}
                        prior.dump(out / 'receipt.json', receipt)
                        case_deadline = min(deadline, time.monotonic() + 20)
                        argv = [str(args.binary.resolve()), str(case), shape, str(seed), str(cadence), observer]
                        code, _ = prior.run(argv, evidence, 'native', case_deadline, commands,
                            dict(os.environ, FABRIC_SCRATCH_ROOT=str(work)))
                        if code:
                            raise RuntimeError(f'{label} native exit {code}')
                        if prior.footprint(case / 'state') > 64 * MIB or prior.footprint(case) > FAILURE_CAP:
                            raise RuntimeError('state/raw case bound exceeded')
                        # Large mutated answer files are owned scratch, so the
                        # persistent category need not reserve duplicate raw pages.
                        grading = case / 'grading'
                        grading.mkdir()
                        verdicts = prior.grade(case, grading, case_deadline, commands)
                        if prior.footprint(case) > FAILURE_CAP:
                            raise RuntimeError('graded whole raw case exceeds128MiB')
                        summary, telemetry = report_metrics(case)
                        if (summary['shape'], summary['period_ms'], summary['seed'], summary['observer']) != (
                                shape, cadence, seed, observer):
                            raise RuntimeError('native cell identity differs from registered invocation')
                        for path in grading.iterdir():
                            if not path.name.endswith('-answer.json'):
                                shutil.copyfile(path, evidence / path.name)
                        prior.dump(evidence / 'metrics.json', summary)
                        prior.dump(evidence / 'oracle-verdicts.json', verdicts)
                        prior.dump(evidence / 'success-file-hashes.json', file_manifest(case))
                        telemetry_bytes = json.dumps(telemetry, sort_keys=True).encode()
                        telemetry_path = evidence / 'telemetry.json.gz'
                        telemetry_path.write_bytes(gzip.compress(telemetry_bytes, mtime=0))
                        if gzip.decompress(telemetry_path.read_bytes()) != telemetry_bytes:
                            raise RuntimeError('telemetry readback mismatch')
                        # Preserve one matched representative; other success
                        # artifacts are explicitly summarized, never called full archives.
                        retained = None
                        if (shape, cadence, seed, observer) == ('skewed3', 2, SEEDS[0], 'events'):
                            retained = prior.archive.preserve(case, evidence, FAILURE_CAP)
                            if (evidence / 'fixture.tar.gz').stat().st_size > 24 * MIB:
                                raise RuntimeError('representative compressed fixture exceeds24MiB')
                        controls = {path.name: {'sha256': prior.sha(path), 'bytes': path.stat().st_size}
                                    for path in grading.glob('*-answer.json')}
                        prior.dump(evidence / 'negative-answer-hashes.json', controls)
                        for path, digest in frozen.items():
                            if prior.sha(Path(path)) != digest:
                                raise RuntimeError('frozen source changed during sweep')
                        if prior.sha(args.binary) != receipt['binary_sha256']:
                            raise RuntimeError('frozen binary changed during sweep')
                        if prior.footprint(out) > SUCCESS_CAP:
                            raise RuntimeError('success evidence exceeds100MiB')
                        after = prior.cgroup()
                        shutil.rmtree(case)
                        receipt['cases'].append({'label': label, 'metrics': summary, 'commands': commands,
                            'after': after, 'retained_full_fixture': retained is not None,
                            'fixture_archive_sha256': retained['archive_sha256'] if retained else None,
                            'telemetry_sha256': prior.sha(telemetry_path), 'scratch_removed': not case.exists()})
                        receipt.pop('active_case', None)
                        receipt['elapsed_s'] = time.monotonic() - started
                        prior.dump(out / 'receipt.json', receipt)
        shutil.rmtree(work)
        receipt.update(state='complete', scratch_removed=not work.exists())
    except BaseException as error:
        receipt.update(state='failed', error=repr(error))
        failure = out / 'failure'
        failure.mkdir(exist_ok=True)
        try:
            if prior.footprint(work) > FAILURE_CAP:
                raise RuntimeError('whole failure raw exceeds128MiB; keep original')
            receipt['failure_preservation'] = prior.archive.preserve(work, failure, FAILURE_CAP)
            if (failure / 'fixture.tar.gz').stat().st_size > FAILURE_CAP:
                raise RuntimeError('whole failure archive exceeds128MiB; keep original')
            shutil.rmtree(work)
            receipt['scratch_removed'] = not work.exists()
        except BaseException as preservation:
            receipt.update(preservation_error=repr(preservation), scratch_removed=False)
        raise
    finally:
        receipt.update(elapsed_s=time.monotonic() - started, after=prior.cgroup(),
                       persistent_bytes=prior.footprint(out))
        prior.dump(out / 'receipt.json', receipt)


if __name__ == '__main__':
    main()
