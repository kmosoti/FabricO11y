#!/usr/bin/env python3
"""Isolated matched small-profile query cells; coordinator executes under containment."""
import argparse
import copy
import gzip
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time

ROOT = Path(__file__).resolve().parents[5]
sys.path.insert(0, str(ROOT / 'tools/bench'))
import observe_dev_small as observation

native = observation.native
PROTOCOL = ROOT / 'docs/experiments/benchmarks/data/readiness-labs-run-01/query/protocol.md'
UNMEASURED = {'status': 'not_applicable', 'reason': 'query-off removes all timed HTTP consumers',
              'samples': 0, 'value': None}


class QueryOff(observation.Observer):
    """Keep sampling and post-timing independent checks; schedule no consumers."""
    def start(self, api, epoch, nodes):
        self.api, self.epoch = api, epoch

    def target(self, tag, sha, source_ns, phase):
        pass

    def finish(self, root, summary, samples, events):
        # The unchanged verifier still grades quiescent pages, custody, source
        # lag and resources. Its empty-population statistics are safe (None),
        # then replaced below to explicitly distinguish unmeasured dimensions.
        super().finish(root, summary, samples, events)
        self.mark_unmeasured(summary)
        native.dump(root / 'summary.json', summary)

    @staticmethod
    def mark_unmeasured(summary):
        inapplicable = ('all_visibility_targets_returned', 'source_to_visible_le_30s',
                        'no_query_errors', 'no_incomplete_answers', 'live_returned_logs_exact')
        summary['not_applicable_gates'] = {name: dict(UNMEASURED) for name in inapplicable}
        for name in inapplicable:
            summary['gates'].pop(name)
        summary['observation']['queries'] = dict(UNMEASURED)
        summary['observation']['visibility'] = dict(UNMEASURED)
        summary['observation']['visibility_missing'] = None
        summary['passed'] = all(summary['gates'].values())


def annotate(root, summary, cell, samples):
    phases = ('normal', 'burst', 'recovery')
    obs = summary['observation']
    phase_cpu = [obs['cpu'].get(p, {}).get('processes', {}).get('server', {}).get('mean_cores') for p in phases]
    offered = [s for s in samples if observation.classify(s['wall_ns'], obs['epoch_ns']) in phases]
    clock_valid = summary['gates'].get('clock_offset_range_le_5ms', False)
    unavailable = {}
    balanced_cpu = None
    if all(v is not None for v in phase_cpu) and clock_valid:
        balanced_cpu = sum(phase_cpu) / len(phase_cpu)
    else:
        unavailable['balanced_phase_server_cores'] = 'clock discontinuity or unavailable offered-phase CPU population'
    peak_rss = None
    if offered and clock_valid:
        peak_rss = max(s['processes'][0]['rss_kib'] for s in offered) / 1024
    else:
        unavailable['offered_phase_peak_server_rss_mib'] = 'clock discontinuity or empty offered-phase resource population'
    balanced = None
    if cell != 'off':
        medians = [obs['queries'][shape][p]['latency_ms']['p50'] for p in phases
                   for shape in ('recent_logs', 'absent_text', 'cpu_metrics')]
        if all(v is not None for v in medians) and clock_valid:
            balanced = sum(medians) / len(medians)
        else:
            unavailable['balanced_phase_shape_median_ms'] = 'clock discontinuity or missing phase/query population'
    summary['lab'] = {'cell': cell, 'query_plan': 'walk' if cell == 'walk' else 'scan',
        'inference': 'single fresh screening cell; optimization not confirmed',
        'visibility_sampling': 'fixed 5-second tags, 250ms polling; phase locked; no population p99 claim',
        'builder_heap': {'value': None, 'reason': 'not instrumented'},
        'metrics': {'balanced_phase_server_cores': balanced_cpu,
                    'balanced_phase_shape_median_ms': balanced,
                    'offered_phase_peak_server_rss_mib': peak_rss},
        'unavailable_metrics': unavailable,
        'cgroup_memory_peak_scope': 'resource samples include preflight through last timed sample; launcher final peak also includes post-timing work',
        'raw_resource_scope': 'one-second timed snapshots; phase CPU omits unsampled boundary intervals'}
    native.dump(root / 'summary.json', summary)


class LabObserver(observation.Observer):
    def __init__(self, cell):
        super().__init__()
        self.cell = cell

    def finish(self, root, summary, samples, events):
        super().finish(root, summary, samples, events)
        annotate(root, summary, self.cell, samples)


class OffObserver(QueryOff):
    def finish(self, root, summary, samples, events):
        super().finish(root, summary, samples, events)
        annotate(root, summary, 'off', samples)


def controls():
    checks = observation.controls()
    off = QueryOff()
    def forbidden(*args):
        raise AssertionError('query-off started HTTP consumer')
    off.start(forbidden, 123, 20)
    off.target('00:0000:00', 'sha', 123, 'normal')
    assert not off.threads and not off.target_rows and not off.queries and off.targets.empty()
    assert type(off).sample is observation.Observer.sample
    assert type(off).quiescent is observation.Observer.quiescent
    off.stop()
    def candidate(exact):
        return {'gates': {'exact_source_logs': exact, 'final_queries_exact': True,
            'all_visibility_targets_returned': False, 'source_to_visible_le_30s': False,
            'no_query_errors': True, 'no_incomplete_answers': True, 'live_returned_logs_exact': True},
            'observation': {}}
    good, missing = candidate(True), candidate(False)
    off.mark_unmeasured(good)
    off.mark_unmeasured(missing)
    assert good['passed'] and not missing['passed'], 'query-off normalization hid missing custody'
    assert good['observation']['visibility']['value'] is None
    assert good['observation']['visibility']['status'] == 'not_applicable'
    assert good['observation']['queries']['samples'] == 0
    checks.update(query_off_no_consumers_or_targets=True,
                  query_off_reuses_resource_sampling_and_postrun_oracle=True,
                  query_off_missing_custody_still_rejected=True,
                  query_off_visibility_explicitly_unmeasured=True)
    fixture = PROTOCOL.parent / 'clock-discontinuity-fixture.json'
    with fixture.open() as stream:
        failed = json.load(stream)
    original_gates = copy.deepcopy(failed['gates'])
    with tempfile.TemporaryDirectory(prefix='query-clock-control-', dir=os.environ['TMPDIR']) as directory:
        annotate(Path(directory), failed, 'scan', [])
    assert failed['gates'] == original_gates and not failed['passed']
    assert all(value is None for value in failed['lab']['metrics'].values())
    assert len(failed['lab']['unavailable_metrics']) == 3
    checks['real_clock_discontinuity_preserves_failure_and_null_metrics'] = True
    return checks


def snapshot(data):
    (data / 'working-tree.diff').write_bytes(subprocess.check_output(['git', 'diff', 'HEAD'], cwd=ROOT))
    names = subprocess.check_output(['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard'], cwd=ROOT)
    with tarfile.open(data / 'source-snapshot.tar.gz', 'x:gz') as archive:
        for name in names.decode().split('\0'):
            p = ROOT / name
            if name and p.is_file() and not name.startswith('docs/experiments/benchmarks/data/') and (
                    p.suffix in ('.rs', '.py') or p.name in ('Cargo.toml', 'Cargo.lock')):
                archive.add(p, arcname=name)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cell', choices=('off', 'scan', 'walk'))
    parser.add_argument('--controls', action='store_true')
    parser.add_argument('--out', type=Path, help='fresh evidence directory; defaults to registered cell directory')
    args = parser.parse_args()
    observation.require_limits()
    checked = controls()
    if args.controls:
        print(json.dumps(checked), flush=True)
        return
    if args.cell is None:
        parser.error('--cell or --controls required')
    data = args.out or PROTOCOL.parent / args.cell
    data = data.resolve()
    if not data.is_relative_to(PROTOCOL.parent.resolve()):
        raise RuntimeError('evidence must stay in owned query lab directory')
    if data.exists():
        raise RuntimeError('evidence directory exists; refusing to overwrite completed/partial cell')
    existing_bytes = native.footprint(PROTOCOL.parent)
    remaining_screen_cells = sum(not (PROTOCOL.parent / cell).exists() for cell in ('off', 'scan', 'walk'))
    projected_bytes = existing_bytes + max(1, remaining_screen_cells) * 16 * 2**20
    if projected_bytes > 50 * 2**20:
        raise RuntimeError('conservative 16MiB/cell evidence projection exceeds 50MiB lab bound')
    cpus = sorted(os.sched_getaffinity(0))
    if len(cpus) < 4:
        raise RuntimeError('four allowed CPUs required')
    tmp = Path(os.environ['TMPDIR']).resolve()
    storage = observation.STORAGE.resolve()
    if not tmp.is_relative_to(storage / 'scratch'):
        raise RuntimeError('TMPDIR must be launcher-owned data-drive scratch')
    work = tmp / ('query-' + data.name)
    if work.exists():
        raise RuntimeError('owned scratch already exists')
    bins = Path(os.environ['CARGO_TARGET_DIR']) / 'release'
    binaries = [bins / 'fabric-server', bins / 'fabric-node', bins / 'examples/server_dump']
    hashes = {str(p.relative_to(bins)): observation.digest(p) for p in binaries}
    if shutil.disk_usage(storage).free < 4 * 2**30:
        raise RuntimeError('data-drive free-space reserve unavailable')
    data.mkdir(parents=True)
    native.dump(data / 'controls.json', checked)
    shutil.copy(PROTOCOL, data / 'protocol.txt')
    snapshot(data)
    group = observation.cgroup()
    native.dump(data / 'environment.json', {'command': sys.argv,
        'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
        'protocol_sha256': observation.digest(PROTOCOL),
        'protocol_commit': 'coordinator must record commit or unavailable-policy-commit limitation',
        'harness_sha256': observation.digest(Path(__file__)),
        'native_sha256': observation.digest(Path(native.__file__)),
        'observer_sha256': observation.digest(Path(observation.__file__)),
        'binaries': hashes, 'server_cpus': cpus[:2], 'client_cpus': cpus[2:4],
        'cgroup': str(group), 'limits': {n: (group / n).read_text().strip() for n in
            ('memory.max', 'memory.high', 'memory.swap.max', 'cpu.max')},
        'storage': str(work), 'free_bytes': shutil.disk_usage(storage).free,
        'evidence_projection_bytes': projected_bytes, 'evidence_cell_reserve_bytes': 16 * 2**20,
        'uname': list(os.uname()), 'cpuinfo': Path('/proc/cpuinfo').read_text(),
        'rustc': subprocess.check_output(['rustc', '-Vv'], text=True),
        'build_settings': {k: os.environ.get(k) for k in ('CARGO_TARGET_DIR', 'RUSTFLAGS', 'CARGO_BUILD_JOBS')}})
    observer = OffObserver() if args.cell == 'off' else LabObserver(args.cell)
    status = 'interrupted'
    began = time.monotonic()
    try:
        os.sched_setaffinity(0, cpus[2:4])
        summary = native.trial(work, bins, 'small', cpus[:2], cpus[2:4], observer,
                               query_plan='walk' if args.cell == 'walk' else 'scan')
        if hashes != {str(p.relative_to(bins)): observation.digest(p) for p in binaries}:
            raise RuntimeError('binary changed during trial')
        status = 'passed' if summary['passed'] else 'failed'
    except BaseException as exc:
        native.dump(data / 'failure.json', {'error': repr(exc)})
        raise
    finally:
        if work.exists():
            observer.archive(work)
            resources = work / 'resources.json'
            if resources.exists():
                with resources.open('rb') as src, gzip.open(work / 'resources.json.gz', 'wb', compresslevel=9) as dst:
                    shutil.copyfileobj(src, dst)
                resources.unlink()
            evidence = [p for p in work.iterdir() if p.is_file() and
                        p.name != 'recovered.jsonl' and p.suffix in ('.json', '.gz', '.err', '.out')]
            retained = native.footprint(PROTOCOL.parent) + sum(p.stat().st_size for p in evidence)
            if retained > 50 * 2**20:
                status = 'failed'
                native.dump(data / 'preservation-failure.json', {'bytes': retained, 'bound_bytes': 50 * 2**20,
                            'reason': 'compact evidence exceeds per-lab bound; scratch retained'})
            else:
                for p in evidence:
                    shutil.copy(p, data / p.name)
            size = native.footprint(work)
            if status == 'passed':
                shutil.rmtree(work)
            native.dump(data / 'cleanup.json', {'status': status, 'work': str(work),
                'removed': not work.exists(), 'logical_bytes': size,
                'elapsed_seconds': time.monotonic() - began,
                'retained_compact_bytes': native.footprint(PROTOCOL.parent),
                'failed_work_moves_with_launcher_tmpdir': status != 'passed'})
    if status != 'passed':
        raise SystemExit(1)


if __name__ == '__main__':
    main()
