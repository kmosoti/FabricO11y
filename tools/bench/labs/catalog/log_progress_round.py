"""Registered progress correction and whole-log-extraction comparison."""
import ast
import json
import os
from pathlib import Path
import shutil
import statistics
import sys
import time

import algorithm_round as previous

common = previous.common
ROOT = common.ROOT
ID = 'catalog-log-progress-round-02'
OUT = ROOT / 'docs/experiments/benchmarks/data' / ID
RUST = ('src/spindle/runtime.rs', 'src/spindle/skip_progress_tests.rs',
        'crates/fabric-server/src/rows.rs', 'crates/fabric-server/src/rows_ownership_tests.rs')
SOURCES = (*RUST, 'src/spindle/runtime_ownership_tests.rs', 'src/spindle/spool.rs',
           'crates/fabric-adapter-linux/src/log_source.rs', 'Cargo.toml', 'Cargo.lock',
           'tools/bench/labs/catalog/log_progress_round.py',
           'tools/bench/labs/catalog/log_progress_repro.py',
           'tools/bench/labs/catalog/coupled_cleanup.py')


def correct_progress(trace):
    passes = trace.get('passes', [])
    return (len(passes) == 4 and trace.get('reopened_after_pass') == 2
        and [p['cursor']['offset'] for p in passes] == [1048576, 2097152, 3145728, 3145746]
        and [p['acked_after'] for p in passes] == [1, 2, 3, 4]
        and [p['returned_cycle']['sequence'] for p in passes] == [1, 2, 3, 4]
        and [p['returned_cycle']['logs'] for p in passes] == [0, 0, 0, 1]
        and passes[1]['returned_cycle']['metrics'] > 0
        and passes[2]['returned_cycle']['metrics'] > 0
        and trace['final']['logs'] == 1 and len(trace['final']['gaps']) == 1
        and trace['final']['acked'] == 4)


def summarize():
    traces = [row['skip_progress_final'] for row in previous.rows(OUT / 'progress.stdout')
              if 'skip_progress_final' in row]
    if len(traces) != 1 or not correct_progress(traces[0]):
        raise RuntimeError('complete post-fix progress trace missing or incorrect')
    wrong = json.loads(json.dumps(traces[0]))
    wrong['passes'][1]['cursor']['offset'] = 1048576
    if correct_progress(wrong):
        raise RuntimeError('progress checker accepted stalled cursor')
    common.dump(OUT / 'progress-trace.json', traces[0])
    raw = previous.index_rows(previous.rows(OUT / 'projection.stdout'),
        ['body_bytes', 'string_attributes_per_log', 'pair', 'owned'],
        {(b, a, p, o) for b in (16, 1024) for a in (0, 8) for p in (1, 2, 3)
         for o in (False, True)})
    cells = []
    for body in (16, 1024):
        for attributes in (0, 8):
            ratios = []
            for pair in (1, 2, 3):
                old = raw[body, attributes, pair, False]
                new = raw[body, attributes, pair, True]
                for row in (old, new):
                    assert row['exact_rows'] and row['repetitions'] == 20
                    assert row['checked_output_rows'] == 20480
                    assert row['batches'] == 8 and row['logs_per_batch'] == 128
                ratios.append(new['whole_extract_wall_ns'] / old['whole_extract_wall_ns'])
            median = statistics.median(ratios)
            cells.append({'body_bytes': body, 'string_attributes': attributes,
                          'paired_ratios': ratios, 'median_ratio': median,
                          'guard_met': median <= (.95 if body == 1024 else 1.10)})
    return {'progress_trace_checked': True, 'stalled_cursor_control_rejected': True,
            'projection_raw_arms': len(raw), 'projection': cells,
            'projection_h1_met': all(cell['guard_met'] for cell in cells),
            'scope': 'progress correctness and whole extraction wall time; no service rate/RSS claim'}


def main():
    common.require_limits()
    OUT.mkdir(exist_ok=False)
    work = Path(os.environ['FABRIC_SCRATCH_ROOT']) / 'log-progress-round'
    work.mkdir()
    env = dict(os.environ, FABRIC_SCRATCH_ROOT=str(work))
    for key in ('FABRIC_ACK_ADVANCE_EXPERIMENT', 'FABRIC_SPILL_WORKSPACE_EXPERIMENT',
                'FABRIC_RUN_MIB_EXPERIMENT', 'FABRIC_BORROWED_LOG_EXPERIMENT'):
        env.pop(key, None)
    deadline = time.monotonic() + 860
    receipt = {'state': 'running', 'commands': [], 'scratch': str(work)}
    def execute(label, argv):
        code = common.profile.run_child(argv, env, OUT / (label + '.stdout'),
            OUT / (label + '.stderr'), deadline, work, OUT)
        receipt['commands'].append({'argv': argv, 'exit': code})
        common.dump(OUT / 'receipt.json', receipt)
        if code:
            raise RuntimeError('command failed: ' + label)
    try:
        for p in SOURCES:
            if p.endswith('.py'):
                ast.parse((ROOT / p).read_text(), filename=p)
        execute('format', ['rustfmt', '--edition', '2024', *RUST])
        frozen = {p: common.sha(ROOT / p) for p in SOURCES}
        receipt['source_sha256'] = frozen
        for p in SOURCES:
            target = OUT / 'source' / p
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / p, target)
        shutil.copyfile(ROOT / 'docs/experiments/benchmarks/catalog-log-progress-protocol.md',
                        OUT / 'protocol.txt')
        shutil.copyfile(ROOT / 'docs/experiments/benchmarks/catalog-log-progress-retry-protocol.md',
                        OUT / 'retry-protocol.txt')
        execute('previous-scratch-cleanup', [sys.executable, '-B',
            'tools/bench/labs/catalog/coupled_cleanup.py', '--id',
            'catalog-log-progress-cleanup-01', '--only-job', 'catalog-log-progress-round-01'])
        execute('progress', ['cargo', 'test', '--offline', '--locked', '-p', 'fabric_o11y',
            '--lib', 'skip_progress_tests::', '--', '--nocapture', '--test-threads=1'])
        execute('collection-ownership', ['cargo', 'test', '--offline', '--locked', '-p',
            'fabric_o11y', '--lib', 'ownership_', '--', '--test-threads=1'])
        execute('projection-controls', ['cargo', 'test', '--offline', '--locked', '-p',
            'fabric-server', '--lib', 'rows::ownership_tests::', '--', '--test-threads=1'])
        execute('projection', ['cargo', 'test', '--offline', '--locked', '--release', '-p',
            'fabric-server', '--lib', 'logs_ownership_timing_probe', '--', '--ignored',
            '--nocapture', '--test-threads=1'])
        common.dump(OUT / 'summary.json', summarize())
        if frozen != {p: common.sha(ROOT / p) for p in SOURCES}:
            raise RuntimeError('comparison source changed')
        allocated = sum(max(p.stat().st_size, p.stat().st_blocks * 512)
                        for p in OUT.rglob('*') if p.is_file())
        if allocated > 1024**2:
            raise RuntimeError('1MiB evidence cap exceeded')
        receipt.update(state='complete', allocated_evidence_bytes=allocated)
        shutil.rmtree(work)
    except BaseException as error:
        receipt.update(state='failed', error=repr(error))
        raise
    finally:
        receipt['scratch_removed'] = not work.exists()
        common.dump(OUT / 'receipt.json', receipt)


if __name__ == '__main__':
    main()
