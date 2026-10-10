"""Finite transition counterexamples and correction; root-cgroup execution only."""
import argparse
import ast
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

import native_lifecycle_run as archive

common = archive.common
ROOT = common.ROOT
SOURCES = (
    'crates/fabric-server/src/lib.rs', 'crates/fabric-server/src/http.rs',
    'crates/fabric-server/src/query.rs', 'crates/fabric-server/src/store.rs',
    'crates/fabric-server/src/sealer.rs', 'crates/fabric-server/src/lifecycle_workers.rs',
    'crates/fabric-server/src/segment.rs',
    'crates/fabric-server/src/http/admission_tests.rs',
    'crates/fabric-server/src/query/snapshot_evidence_tests.rs',
    'crates/fabric-server/tests/cancellation.rs',
    'crates/fabric-server/tests/query_snapshot_transition.rs',
    'crates/fabric-server/tests/admission.rs',
    'tools/qualification/query_oracle.py', 'Cargo.lock',
    'tools/bench/labs/catalog/transition_ownership.py',
)


def cancellation_counterexample(rows):
    abort = [r['cancellation_trace'] for r in rows if
             r.get('cancellation_trace', {}).get('case') == 'abort_after_listening']
    bind = [r['cancellation_trace'] for r in rows if
            r.get('cancellation_trace', {}).get('case') == 'occupied_port_bind_failure']
    if not abort or not bind:
        return False
    a, b = abort[-1], bind[-1]
    attempts = a.get('reopen_attempts', [])
    return (a.get('pre_start_store_open') is True and bool(a.get('listening'))
            and a.get('task_cancelled') is True and len(attempts) >= 2
            and all(t.get('opened') is False and t.get('kind') == 'WouldBlock' for t in attempts)
            and b.get('serve_error', {}).get('kind') == 'AddrInUse'
            and any(t.get('opened') is True for t in b.get('reopen_attempts', [])))


def snapshot_counterexample(rows):
    failed = {f'{stage}-{i}' for stage in ('published', 'restarted') for i in (0, 1)}
    good = {f'{stage}-{i}' for stage in ('tail', 'append', 'fresh', 'missing-control', 'retention')
            for i in (0, 1)}
    return (len(rows) == len(failed | good) and {name for name, _ in rows} == failed | good
            and all(passed is (name in good) for name, passed in rows))


def admission_counterexample(trace):
    statuses = {'batch_after_partial_cancellation': 401, 'batch_while_query_pool_full': 401,
                'query_while_batch_pool_full': 200, 'query_after_partial_cancellation': 200}
    return (trace.get('batch_interim_continue_witnesses') == 16
            and trace.get('query_interim_continue_witnesses') == 2
            and all(trace.get(name, {}).get('status') == status for name, status in statuses.items())
            and all(trace.get(name, {}).get('kind') in ('WouldBlock', 'TimedOut')
                    and bool(trace.get(name, {}).get('io_error'))
                    and 'status' not in trace.get(name, {})
                    for name in ('batch_excess_before_body_complete', 'query_excess_before_body_complete')))


def main():
    common.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mode', choices=('repro', 'admission', 'fixed', 'checks'), required=True)
    parser.add_argument('--checks-attempt', choices=(1, 2), type=int, default=1)
    args = parser.parse_args()
    ids = {'repro': 'catalog-transition-repro-01', 'admission': 'catalog-admission-repro-01',
           'fixed': 'catalog-transition-fixed-01', 'checks': 'catalog-transition-checks-01'}
    name = ids[args.mode]
    if args.mode == 'checks' and args.checks_attempt == 2:
        name = 'catalog-transition-checks-02'
    out = ROOT / 'docs/experiments/benchmarks/data' / name
    out.mkdir(exist_ok=False)
    work = Path(os.environ['FABRIC_SCRATCH_ROOT']) / name
    work.mkdir()
    env = dict(os.environ, FABRIC_SCRATCH_ROOT=str(work))
    for key in ('FABRIC_ACK_ADVANCE_EXPERIMENT', 'FABRIC_SPILL_WORKSPACE_EXPERIMENT',
                'FABRIC_RUN_MIB_EXPERIMENT', 'FABRIC_BORROWED_LOG_EXPERIMENT',
                'FABRIC_NATIVE_EVIDENCE', 'FABRIC_QUERY_SNAPSHOT_EVIDENCE'):
        env.pop(key, None)
    if args.mode == 'fixed':
        env['FABRIC_QUERY_SNAPSHOT_EVIDENCE'] = '1'
    deadline = time.monotonic() + {'repro': 150, 'admission': 150, 'fixed': 210, 'checks': 560}[args.mode]
    receipt = {'state': 'running', 'commands': [], 'scratch': str(work)}
    common.dump(out / 'receipt.json', receipt)

    def execute(label, argv):
        code = common.profile.run_child(argv, env, out / (label + '.stdout'),
            out / (label + '.stderr'), deadline, work, out)
        receipt['commands'].append({'argv': argv, 'exit': code})
        common.dump(out / 'receipt.json', receipt)
        return code

    def test(target):
        return execute(target, ['cargo', 'test', '--offline', '--locked', '-p', 'fabric-server',
            '--test', target, '--', '--nocapture', '--test-threads=1'])

    try:
        ast.parse(Path(__file__).read_text())
        formatting = (['crates/fabric-server/tests/cancellation.rs',
                       'crates/fabric-server/tests/query_snapshot_transition.rs'] if args.mode == 'repro'
                      else ['crates/fabric-server/tests/admission.rs'] if args.mode == 'admission' else [])
        if formatting and execute('format', ['rustfmt', '--edition', '2024', *formatting]):
            raise RuntimeError('formatting failed')
        if args.mode == 'fixed' and execute('format', ['cargo', 'fmt', '--all']):
            raise RuntimeError('candidate formatting failed')
        if args.mode == 'checks' and args.checks_attempt == 2:
            if execute('format', ['cargo', 'fmt', '--all']):
                raise RuntimeError('relocated test formatting failed')
            unit = 'fabric-work-e051d6137a624086836988a3644fd7fa'
            status = subprocess.run(['systemctl', '--user', 'is-active', unit + '.service'],
                                    capture_output=True, text=True, timeout=10)
            if status.returncode not in (3, 4) or status.stdout.strip() not in ('inactive', 'failed', 'unknown'):
                raise RuntimeError('previous unit not confirmed inactive')
            held = Path('/run/media/kmosoti/data/FabricO11y/evidence') / unit
            previous = out / 'previous-failure'
            previous.mkdir()
            if held.is_symlink() or not held.is_dir():
                raise RuntimeError('expected previous failure tree unavailable')
            preserved = archive.preserve(held, previous, 512 * 1024)
            shutil.rmtree(held)
            receipt['previous_failure_cleanup'] = {'source': str(held),
                'unit_state': status.stdout.strip(), 'archive': 'previous-failure/fixture.tar.gz',
                'exact_readback': preserved['exact_readback'],
                'decoded_file_bytes': preserved['decoded_file_bytes'], 'removed': not held.exists()}
        sources = [path for path in SOURCES if (ROOT / path).exists()]
        frozen = {path: common.sha(ROOT / path) for path in sources}
        receipt['source_sha256'] = frozen
        receipt['archive_controls_rejected'] = archive.archive_controls(work)
        if args.mode == 'repro':
            code = test('cancellation')
            rows = archive.parsing.rows(out / 'cancellation.stdout')
            if code != 101 or not cancellation_counterexample(rows):
                raise RuntimeError('exact cancellation counterexample not reproduced')
            changed = json.loads(json.dumps(rows))
            for row in changed:
                if row.get('cancellation_trace', {}).get('case') == 'abort_after_listening':
                    row['cancellation_trace']['reopen_attempts'] = [{'opened': True}]
            if cancellation_counterexample(changed):
                raise RuntimeError('classifier accepted released journal')
            code = test('query_snapshot_transition')
            paths = list(work.glob('query-snapshot-transition-*/outcomes-before-assertion.json'))
            if code != 101 or len(paths) != 1 or not snapshot_counterexample(json.loads(paths[0].read_text())):
                raise RuntimeError('exact snapshot counterexample not reproduced')
            repaired = [[name, True] for name, _ in json.loads(paths[0].read_text())]
            if snapshot_counterexample(repaired):
                raise RuntimeError('classifier accepted correct snapshot')
            receipt.update(state='counterexamples_reproduced', rust_checks='failed',
                           classifier_controls_rejected=['released_journal', 'correct_snapshot'])
        elif args.mode == 'admission':
            code = test('admission')
            rows = archive.parsing.rows(out / 'admission.stdout')
            traces = [r['admission_trace'] for r in rows if 'admission_trace' in r]
            if code != 101 or len(traces) != 1 or not admission_counterexample(traces[0]):
                raise RuntimeError('exact admission counterexample not reproduced')
            changed = json.loads(json.dumps(traces[0]))
            changed['batch_excess_before_body_complete'] = {'status': 503, 'retry_after': '1'}
            if admission_counterexample(changed):
                raise RuntimeError('classifier accepted bounded admission')
            receipt.update(state='counterexample_reproduced', rust_exit=code,
                           rust_checks='failed', admitted_control_rejected=True)
        elif args.mode == 'fixed':
            for target in ('cancellation', 'query_snapshot_transition', 'admission', 'startup', 'delivery'):
                if test(target):
                    raise RuntimeError('corrected regression failed: ' + target)
            if execute('ownership-units', ['cargo', 'test', '--offline', '--locked', '-p',
                    'fabric-server', '--lib', 'lifecycle_workers::tests', '--', '--nocapture', '--test-threads=1']):
                raise RuntimeError('ownership unit controls failed')
            if execute('admission-units', ['cargo', 'test', '--offline', '--locked', '-p',
                    'fabric-server', '--lib', 'http::admission_tests', '--', '--nocapture', '--test-threads=1']):
                raise RuntimeError('admission unit controls failed')
            if execute('snapshot-units', ['cargo', 'test', '--offline', '--locked', '-p',
                    'fabric-server', '--lib', 'query::snapshot_evidence_tests', '--', '--nocapture', '--test-threads=1']):
                raise RuntimeError('snapshot unit control failed')
            receipt['state'] = 'passed'
        else:
            for profile in ('fast', 'documentation'):
                if execute(profile, [sys.executable, '-B', 'tools/bench/labs/completion/checks.py',
                        '--profile', profile, '--id', name + ('-docs' if profile == 'documentation' else '')]):
                    raise RuntimeError('verification failed: ' + profile)
            receipt['state'] = 'passed'
        if frozen != {path: common.sha(ROOT / path) for path in sources}:
            raise RuntimeError('source changed during execution')
        if args.mode != 'checks':
            receipt['fixture'] = archive.preserve(work, out, 2 * 1024**2)
        shutil.rmtree(work)
    except BaseException as error:
        receipt.update(state='failed', error=repr(error))
        if work.exists() and not (out / 'fixture.tar.gz').exists():
            try:
                receipt['failure_fixture'] = archive.preserve(work, out, 2 * 1024**2)
                shutil.rmtree(work)
            except BaseException as preservation_error:
                receipt['preservation_error'] = repr(preservation_error)
        raise
    finally:
        if args.mode == 'checks':
            import coupled_bun_preserve as bun
            target = Path(os.environ['TMPDIR']) / 'bun'
            if target.exists():
                stored = bun.BASE / bun.ARCHIVE
                if bun.sha(stored) != bun.ARCHIVE_SHA:
                    raise RuntimeError('canonical Bun archive changed; retain scratch')
                bun.compare(stored, bun.MEMBER, target, bun.PAYLOAD_SHA, bun.PAYLOAD_BYTES)
                bun.persist(out / 'bun-reference.json', {'source': str(target),
                    'archive': str(stored.relative_to(ROOT)), 'archive_sha256': bun.ARCHIVE_SHA,
                    'member': bun.MEMBER, 'decoded_sha256': bun.PAYLOAD_SHA,
                    'bytes': bun.PAYLOAD_BYTES, 'exact_readback': True})
                target.unlink()
            receipt['bun_copy_removed'] = not target.exists()
        receipt['scratch_removed'] = not work.exists()
        common.dump(out / 'receipt.json', receipt)
        allocated = sum(max(path.stat().st_size, path.stat().st_blocks * 512)
                        for path in out.rglob('*') if path.is_file())
        if allocated > (512 * 1024 if args.mode == 'checks' else 3 * 1024**2):
            raise RuntimeError('registered driver evidence cap exceeded')


if __name__ == '__main__':
    main()
