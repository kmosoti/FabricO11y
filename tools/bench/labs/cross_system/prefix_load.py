"""Bounded native prefix-progress comparison; invoked by the root allocation runner.

This driver builds nothing. A frozen release binary, registered protocol and
allocation are required. Historical campaign receipts are read and retained.
"""
import argparse
import gzip
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
sys.path.insert(0, str(ROOT / 'tools/bench/labs/catalog'))
from resource_group import require_limits
import native_lifecycle_run as archive

MIB = 1024**2
CAP = 256 * MIB


def dump(path, value):
    path.write_text(json.dumps(value, sort_keys=True, indent=2) + '\n')


def sha(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(65536), b''):
            digest.update(block)
    return digest.hexdigest()


def footprint(root):
    total = 0
    for path in root.rglob('*'):
        if path.is_symlink() or not (path.is_file() or path.is_dir()):
            raise RuntimeError('unexpected owned entry; preserve original')
        if path.is_file():
            total += path.stat().st_size
    return total


def cgroup():
    relative = next(line[3:] for line in Path('/proc/self/cgroup').read_text().splitlines()
                    if line.startswith('0::'))
    group = Path('/sys/fs/cgroup') / relative.lstrip('/')
    return {'path': relative, **{name: (group / name).read_text().strip() for name in
            ('memory.current', 'memory.peak', 'memory.events', 'memory.stat',
             'cpu.stat', 'io.stat', 'memory.max', 'memory.high', 'memory.swap.max')}}


def run(argv, root, label, deadline, commands, env=None):
    stdout, stderr = root / (label + '.stdout'), root / (label + '.stderr')
    started = time.monotonic()
    with stdout.open('xb') as out, stderr.open('xb') as err:
        process = subprocess.Popen(argv, cwd=ROOT, stdout=out, stderr=err,
                                   start_new_session=True, env=env)
        try:
            while process.poll() is None:
                if time.monotonic() >= deadline:
                    raise TimeoutError(label + ' deadline')
                if stdout.stat().st_size + stderr.stat().st_size > MIB:
                    raise RuntimeError(label + ' output exceeds1MiB')
                time.sleep(.02)
        except BaseException:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            commands.append({'argv': argv, 'exit': process.returncode,
                             'elapsed_s': time.monotonic() - started, 'interrupted': True})
            raise
    commands.append({'argv': argv, 'exit': process.returncode,
                     'elapsed_s': time.monotonic() - started})
    return process.returncode, stdout


def grade(work, evidence, deadline, commands):
    oracle = str(ROOT / 'tools/qualification/query_oracle.py')
    verdicts = []
    for query in sorted(work.glob('*-query.json')):
        label = query.name[:-len('-query.json')]
        stage = label.split('-', 1)[0]
        answer = work / (label + '-answer.json')
        argv = [sys.executable, '-B', oracle, '--records', str(work / (stage + '-records.jsonl')),
                '--query', str(query), '--answer', str(answer)]
        code, stdout = run(argv, evidence, label + '-oracle', deadline, commands)
        verdict = json.loads(stdout.read_text())
        if code != 0 or verdict.get('passed') is not True:
            raise RuntimeError(label + ' independent oracle rejected answer')
        verdicts.append({'label': label, 'exit': code, 'verdict': verdict})
    if len(verdicts) != 8:
        raise RuntimeError('pre/post Scan/Walk broad/selective chains missing')
    source = work / 'post-Scan-broad-answer.json'
    pages = json.loads(source.read_text())
    for mutation in ('missing', 'duplicate'):
        bad = json.loads(source.read_text())
        if mutation == 'missing':
            bad[0]['rows'].pop()
        else:
            bad[0]['rows'].append(pages[0]['rows'][0])
        path = evidence / (mutation + '-answer.json')
        dump(path, bad)
        argv = [sys.executable, '-B', oracle, '--records', str(work / 'post-records.jsonl'),
                '--query', str(work / 'post-Scan-broad-query.json'), '--answer', str(path)]
        code, stdout = run(argv, evidence, mutation + '-oracle', deadline, commands)
        verdict = json.loads(stdout.read_text())
        if code == 0 or verdict.get('passed') is not False:
            raise RuntimeError('oracle accepted representative ' + mutation + ' defect')
        verdicts.append({'label': mutation, 'exit': code, 'verdict': verdict})
    # Each full chain is compared as well as independently graded.
    for stage in ('pre', 'post'):
        for shape in ('broad', 'selective'):
            scan = json.loads((work / f'{stage}-Scan-{shape}-answer.json').read_text())
            walk = json.loads((work / f'{stage}-Walk-{shape}-answer.json').read_text())
            if scan != walk:
                raise RuntimeError('Scan/Walk whole-chain disagreement')
    return verdicts


def metrics(work):
    result = json.loads((work / 'result.json').read_text())
    timeline = json.loads((work / 'timeline.json').read_text())
    acks = json.loads((work / 'acks.json').read_text())
    bounds = []
    for label, size in result['initial_sizes']:
        last_present = 0
        first_absent = None
        publication = None
        previous_end = 0
        for event in timeline:
            present = label in [pair[0] for pair in event['journals']]
            if present:
                last_present = event['ns_begin']
            elif first_absent is None:
                first_absent = event['ns']
            if label in event['segments'] and publication is None:
                publication = [previous_end, event['ns']]
            previous_end = event['ns_begin']
        if first_absent is None or publication is None:
            raise RuntimeError('publication/deletion transition not observed')
        bounds.append({'label': label, 'journal_bytes': size,
                       'release_ns': [last_present, first_absent],
                       'publication_ns': publication,
                       'publication_to_release_ns': [max(0, last_present-publication[1]),
                                                     max(0, first_absent-publication[0])]})
    # Three initial files are the matched finite backlog. Newly arriving active
    # bytes are separate; this is not a steady-state capacity measurement.
    integral = [sum(row['journal_bytes'] * row['release_ns'][i] for row in bounds)
                for i in (0, 1)]
    accepted = [row for row in acks if row['answer'] == 'Ack(4)']
    unavailable = [row for row in acks if row['answer'] == 'Unavailable']
    if len(accepted) + len(unavailable) != result['offered']:
        raise RuntimeError('unknown or conflicting ACK acceptance')
    latencies = sorted(row['latency_ns'] for row in accepted)
    if not latencies:
        raise RuntimeError('no successful live ACK observation')
    ticks = sum(result['process_after'][name] - result['process_before'][name]
                for name in ('utime_ticks', 'stime_ticks'))
    sampled_rss_kib = [int(line.split()[1]) for event in timeline
                       for line in event['process']['rss_status'] if line.startswith('VmRSS:')]
    sampled_hwm_kib = [int(line.split()[1]) for event in timeline
                       for line in event['process']['rss_status'] if line.startswith('VmHWM:')]
    return {**result, 'release_observation_bounds': bounds,
            'initial_backlog_byte_ns_bounds': integral, 'unavailable': len(unavailable),
            'accepted_bytes': sum(row['bytes'] for row in accepted),
            'ack_p50_ns': latencies[math.ceil(len(latencies)*.5)-1],
            'ack_p99_ns': latencies[math.ceil(len(latencies)*.99)-1],
            'ack_max_ns': latencies[-1], 'process_cpu_ticks': ticks,
            'clk_tck': os.sysconf('SC_CLK_TCK'),
            'max_offer_lateness_ns': max(row['offered_ns']-row['target_ns'] for row in acks),
            'sampled_schedule_rss_peak_bytes': max(sampled_rss_kib)*1024,
            'whole_process_highwater_includes_startup_bytes': max(sampled_hwm_kib)*1024,
            'rss_samples': [event['process']['rss_status'] for event in timeline],
            'natural_skew_observed': bounds[0]['publication_ns'][1] < bounds[1]['publication_ns'][0],
            'query_oracle': 'eight exact pre/post paginated chains; two rejected mutations',
            'scope': 'native Store/intake+sealer; no HTTP/TLS; not sustained throughput or qualification'}


def main():
    if os.environ.get('FABRIC_CROSS_SYSTEM_COORDINATED') != '1':
        raise RuntimeError('prefix runner requires inherited coordinator lock ownership marker')
    require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--protocol', type=Path, required=True)
    parser.add_argument('--allocation', type=Path, required=True)
    parser.add_argument('--case-timeout-s', type=int, default=20)
    args = parser.parse_args()
    if args.case_timeout_s != 20:
        parser.error('registered case deadline is20s')
    for path in (args.binary, args.protocol, args.allocation):
        if not path.is_file() or path.is_symlink():
            raise RuntimeError('required frozen binary/prospective scope unavailable')
    started = time.monotonic()
    out = args.out.resolve()
    out.mkdir(parents=True)
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True)
    work = scratch / out.name
    work.mkdir()
    receipt = {'state': 'running', 'started_unix_ns': time.time_ns(), 'scratch': str(work),
               'commands': [], 'cases': [], 'before': cgroup(), 'case_deadline_s': 20,
               'round_deadline_s': 600, 'compressed_evidence_cap_bytes': CAP,
               'clock_ticks_per_s': os.sysconf('SC_CLK_TCK'), 'binary_sha256': sha(args.binary)}
    coord = ROOT / 'docs/experiments/benchmarks/data/lab-completion-run-01/coordinator'
    prior = [json.loads(p.read_text()) for p in coord.glob('*/receipt.json')]
    receipt['historical_campaign_elapsed_s'] = sum(r.get('elapsed_s', 0) for r in prior)
    sources = [Path(__file__).resolve(), ROOT / 'crates/fabric-server/examples/prefix_load_probe.rs',
               ROOT / 'crates/fabric-server/src/sealer.rs', ROOT / 'crates/fabric-server/src/store.rs',
               ROOT / 'crates/fabric-server/src/segment.rs', ROOT / 'crates/fabric-server/src/segment/bounded.rs',
               ROOT / 'tools/qualification/query_oracle.py', args.protocol, args.allocation]
    receipt['source_sha256'] = {str(path): sha(path) for path in sources}
    frozen = receipt['source_sha256'].copy()
    for i, path in enumerate(sources):
        raw = path.read_bytes()
        dest = out / f'source-{i:02}.gz'
        dest.write_bytes(gzip.compress(raw, mtime=0))
        if gzip.decompress(dest.read_bytes()) != raw:
            raise RuntimeError('source readback mismatch')
    dump(out / 'receipt.json', receipt)
    round_deadline = started + 600
    current = None
    try:
        receipt['rejected_archive_controls'] = archive.archive_controls(work)
        for shape in ('balanced3', 'skewed3', 'worker1'):
            for seed in (2703204353, 2703204354, 2703204355):
                arms = ('baseline', 'candidate') if seed % 2 else ('candidate', 'baseline')
                for arm in arms:
                    if footprint(out) > CAP - 16*MIB:
                        raise RuntimeError('operations256MiB evidence reserve exhausted')
                    label = f'{shape}-{seed}-{arm}'
                    evidence = out / label
                    evidence.mkdir()
                    case = work / label
                    current = case
                    deadline = min(round_deadline, time.monotonic() + 20)
                    before = cgroup()
                    commands = []
                    receipt['active_case'] = {'label': label, 'commands': commands,
                                              'scratch': str(case), 'before': before}
                    # Diagnostic delay is forbidden in the production comparison.
                    if os.environ.get('FABRIC_PREFIX_DIAGNOSTIC_DELAY_MS', '0') != '0':
                        raise RuntimeError('fixture-only delay cannot enter matched native comparison')
                    argv = [str(args.binary.resolve()), str(case), arm, shape, str(seed)]
                    code, _ = run(argv, evidence, 'native', deadline, commands,
                                  dict(os.environ, FABRIC_SCRATCH_ROOT=str(work)))
                    receipt['commands'].extend(commands)
                    dump(out / 'receipt.json', receipt)
                    if code:
                        raise RuntimeError(label + ' native exit ' + str(code))
                    if footprint(case / 'state') > 64*MIB or footprint(case) > 128*MIB:
                        raise RuntimeError('owned case exceeds registered raw cap')
                    verdicts = grade(case, evidence, deadline, commands)
                    report = metrics(case)
                    dump(evidence / 'metrics.json', report)
                    dump(evidence / 'oracle-verdicts.json', verdicts)
                    retained = archive.preserve(case, evidence, 128*MIB)
                    if (evidence / 'fixture.tar.gz').stat().st_size > 16*MIB:
                        raise RuntimeError('case compressed fixture exceeds16MiB')
                    after = cgroup()
                    for path, digest in frozen.items():
                        if sha(Path(path)) != digest:
                            raise RuntimeError('frozen source changed during comparison')
                    shutil.rmtree(case)
                    receipt['cases'].append({'label': label, 'metrics': report, 'before': before,
                                             'after': after, 'retained': retained,
                                             'scratch_removed': not case.exists(), 'commands': commands})
                    receipt['elapsed_s'] = time.monotonic() - started
                    receipt.pop('active_case', None)
                    dump(out / 'receipt.json', receipt)
                    current = None
        shutil.rmtree(work)
        receipt.update(state='complete', scratch_removed=not work.exists())
    except BaseException as error:
        receipt.update(state='failed', error=repr(error))
        # Preserve the entire owned tree, not just telemetry, before cleanup.
        # If a bound/readback fails, retain the original tree and fail closed.
        failure = out / 'failure'
        failure.mkdir(exist_ok=True)
        try:
            if footprint(work) > 128*MIB:
                raise RuntimeError('raw failure exceeds128MiB; retain original')
            receipt['failure_preservation'] = archive.preserve(work, failure, 128*MIB)
            shutil.rmtree(work)
            receipt['scratch_removed'] = not work.exists()
        except BaseException as preservation:
            receipt['preservation_error'] = repr(preservation)
            receipt['scratch_removed'] = False
        raise
    finally:
        receipt['elapsed_s'] = time.monotonic() - started
        receipt['after'] = cgroup()
        receipt['persistent_bytes'] = footprint(out)
        dump(out / 'receipt.json', receipt)


if __name__ == '__main__':
    # run_job owns readiness-lab.lock before exporting the coordinator marker.
    # Acquiring that lock again in this child rejects its already-owned campaign.
    main()
