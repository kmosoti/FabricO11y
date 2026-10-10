"""Registered bounded range-evidence screen and optional production verification."""
import argparse
import ast
import json
import os
from pathlib import Path
import shutil
import sys
import time

import native_lifecycle_run as archive
import range_evidence_summary as numerical

common = archive.common
ROOT = common.ROOT
EXAMPLE = 'crates/fabric-server/examples/range_evidence_probe.rs'
SOURCES = (EXAMPLE, 'crates/fabric-server/src/rows.rs',
           'crates/fabric-server/src/rows_freshness_tests.rs',
           'crates/fabric-server/src/query.rs', 'crates/fabric-server/src/segment.rs',
           'crates/fabric-server/tests/query_snapshot_transition.rs',
           'crates/fabric-server/src/query/snapshot_evidence_tests.rs',
           'tools/qualification/query_oracle.py', 'Cargo.lock',
           'tools/bench/labs/catalog/range_evidence.py',
           'tools/bench/labs/catalog/range_evidence_summary.py',
           'crates/fabric-server/examples/borrowed_range_probe.rs',
           'tools/bench/labs/catalog/borrowed_range_summary.py')


def main():
    common.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mode', choices=('screen', 'fixed', 'borrowed', 'checks'), required=True)
    args = parser.parse_args()
    borrowed = args.mode == 'borrowed'
    name = 'catalog-range-borrowed-01' if borrowed else f'catalog-range-evidence-{args.mode}-01'
    example = 'borrowed_range_probe' if borrowed else 'range_evidence_probe'
    out = ROOT / 'docs/experiments/benchmarks/data' / name
    out.mkdir(exist_ok=False)
    work = Path(os.environ['FABRIC_SCRATCH_ROOT']) / name
    work.mkdir()
    env = dict(os.environ, FABRIC_SCRATCH_ROOT=str(work))
    for key in ('FABRIC_ACK_ADVANCE_EXPERIMENT', 'FABRIC_SPILL_WORKSPACE_EXPERIMENT',
                'FABRIC_RUN_MIB_EXPERIMENT', 'FABRIC_BORROWED_LOG_EXPERIMENT',
                'FABRIC_NATIVE_EVIDENCE', 'FABRIC_QUERY_SNAPSHOT_EVIDENCE'):
        env.pop(key, None)
    if args.mode in ('fixed', 'borrowed'):
        env['FABRIC_QUERY_SNAPSHOT_EVIDENCE'] = '1'
    deadline = time.monotonic() + {'screen': 270, 'fixed': 210, 'borrowed': 210, 'checks': 315}[args.mode]
    receipt = {'state': 'running', 'commands': [], 'scratch': str(work)}
    common.dump(out / 'receipt.json', receipt)

    def execute(label, argv):
        code = common.profile.run_child(argv, env, out / (label + '.stdout'),
            out / (label + '.stderr'), deadline, work, out)
        receipt['commands'].append({'argv': argv, 'exit': code})
        common.dump(out / 'receipt.json', receipt)
        if code:
            raise RuntimeError('command failed: ' + label)

    try:
        for source in (Path(__file__), ROOT / SOURCES[-1], ROOT / SOURCES[-3]):
            ast.parse(source.read_text())
        if args.mode == 'screen':
            execute('format', ['rustfmt', '--edition', '2024', EXAMPLE, SOURCES[2]])
        elif args.mode in ('fixed', 'borrowed'):
            execute('format', ['cargo', 'fmt', '--all'])
        frozen = {path: common.sha(ROOT / path) for path in SOURCES}
        receipt['source_sha256'] = frozen
        receipt['archive_controls_rejected'] = archive.archive_controls(work)
        if args.mode == 'checks':
            for profile in ('fast', 'documentation'):
                execute(profile, [sys.executable, '-B', 'tools/bench/labs/completion/checks.py',
                    '--profile', profile, '--id', name + ('-docs' if profile == 'documentation' else '')])
        else:
            receipt['binary_sha256'] = {}
            for mode in ('plain', 'counted'):
                build = ['cargo', 'build', '--offline', '--locked', '--release', '-p',
                         'fabric-server', '--example', example]
                if mode == 'counted':
                    build += ['--features', 'responsibility-alloc-probe']
                execute('build-' + mode, build)
                binary = Path(os.environ['CARGO_TARGET_DIR']) / 'release/examples' / example
                digest = common.sha(binary)
                receipt['binary_sha256'][mode] = digest
                execute(mode, [str(binary), str(work / mode)])
                if common.sha(binary) != digest:
                    raise RuntimeError('binary changed during probe')
            raw = {mode: archive.parsing.rows(out / (mode + '.stdout'))
                   for mode in ('plain', 'counted')}
            receipt['stdout_sha256'] = {mode: common.sha(out / (mode + '.stdout')) for mode in raw}
            if borrowed:
                import borrowed_range_summary as grader
            else:
                grader = numerical
            summary = grader.summarize(raw['plain'], raw['counted'])
            common.dump(out / 'summary.json', summary)
            receipt['numerical_controls_rejected'] = grader.negative_controls(raw['plain'], raw['counted'])
            receipt['nomination'] = summary['nomination']
            if args.mode in ('fixed', 'borrowed'):
                for target in ('query_snapshot_transition',):
                    execute(target, ['cargo', 'test', '--offline', '--locked', '-p',
                        'fabric-server', '--test', target, '--', '--nocapture', '--test-threads=1'])
                for label, selector in (('freshness-units', 'rows::freshness_tests'),
                                        ('snapshot-units', 'query::snapshot_evidence_tests')):
                    execute(label, ['cargo', 'test', '--offline', '--locked', '-p',
                        'fabric-server', '--lib', selector, '--', '--nocapture', '--test-threads=1'])
                if borrowed:
                    execute('workspace-test-build', ['cargo', 'test', '--workspace', '--locked',
                        '--offline', '--all-features', '--no-run'])
            receipt['fixture'] = archive.preserve(work, out, 2 * 1024**2)
        if frozen != {path: common.sha(ROOT / path) for path in SOURCES}:
            raise RuntimeError('source changed during execution')
        receipt['state'] = 'passed'
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
        cap = 512 * 1024 if args.mode == 'checks' else (2 if borrowed else 3) * 1024**2
        receipt['driver_evidence_allocated_bytes'] = allocated
        receipt['driver_evidence_cap_bytes'] = cap
        if allocated > cap:
            receipt.update(state='failed', error='registered driver evidence cap exceeded')
            common.dump(out / 'receipt.json', receipt)
            raise RuntimeError('registered driver evidence cap exceeded')
        common.dump(out / 'receipt.json', receipt)


if __name__ == '__main__':
    main()
