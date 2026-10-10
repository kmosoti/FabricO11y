#!/usr/bin/env python3
"""Registered delivery-position and observer screen; coordinator executes once."""
import gzip
import json
import os
from pathlib import Path
import shutil
import statistics
import sys
import time

import coupled_overlap as common

ROOT = common.ROOT
OUT = ROOT / 'docs/experiments/benchmarks/data/catalog-delivery-read-01'
OLD = ROOT / 'docs/experiments/benchmarks/data/catalog-overlap-freeze-01'
SOURCES = ['src/spindle/spool.rs', 'src/spindle/spool_read_probe.rs',
           'src/spindle/runtime.rs', 'examples/coupled_overlap_node.rs',
           'tools/bench/labs/catalog/delivery_read.py',
           'tools/bench/labs/catalog/coupled_overlap.py',
           'tools/qualification/query_oracle.py', 'Cargo.lock']


def main():
    common.require_limits()
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    if not scratch.is_relative_to(common.STORAGE / 'scratch'):
        raise RuntimeError('owned mounted scratch required')
    OUT.mkdir(exist_ok=False)
    work = scratch / 'delivery-read'
    work.mkdir()
    deadline = time.monotonic() + 860
    commands, executables, arms = [], {}, []
    complete = False
    env = dict(os.environ, FABRIC_BORROWED_LOG_EXPERIMENT='0',
               FABRIC_ACK_ADVANCE_EXPERIMENT='0')
    for key in ('FABRIC_SPILL_WORKSPACE_EXPERIMENT', 'FABRIC_RUN_MIB_EXPERIMENT'):
        env.pop(key, None)

    def run(label, argv):
        rc = common.profile.run_child(argv, env, OUT / (label + '.stdout'),
                                      OUT / (label + '.stderr'), deadline, work, OUT)
        commands.append({'label': label, 'argv': argv, 'exit': rc,
                         'ack_selector': env['FABRIC_ACK_ADVANCE_EXPERIMENT']})
        common.dump(OUT / 'commands.json', commands)
        if rc:
            raise RuntimeError(label + ' failed; preserve original output')

    def expand(archive, info, destination):
        if common.sha(archive) != info['archive_sha256']:
            raise RuntimeError('retained binary archive changed')
        with gzip.open(archive, 'rb') as source, destination.open('xb') as target:
            shutil.copyfileobj(source, target)
        if common.sha(destination) != info['decoded_sha256'] or destination.stat().st_size != info['decoded_bytes']:
            raise RuntimeError('retained binary decoded bytes changed')
        destination.chmod(0o700)
        executables[str(destination)] = dict(info, archive=str(archive.relative_to(ROOT)))

    def evidence_cap():
        paths = [p.stat() for p in OUT.rglob('*') if p.is_file()]
        if max(sum(s.st_size for s in paths), sum(s.st_blocks * 512 for s in paths)) > 16 * 2**20:
            raise RuntimeError('registered 16MiB evidence limit')

    try:
        run('format', ['rustfmt', '--edition', '2024', *SOURCES[:4]])
        # run_job preserves the pre-format tree; retain exact measured sources too.
        for name in SOURCES:
            destination = OUT / 'source' / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, destination)
        source_hashes = {name: common.sha(ROOT / name) for name in SOURCES}
        common.dump(OUT / 'source-hashes.json', source_hashes)
        shutil.copyfile(ROOT / 'docs/experiments/benchmarks/catalog-delivery-read-protocol.md', OUT / 'protocol.txt')
        for selector in ('0', '1'):
            env['FABRIC_ACK_ADVANCE_EXPERIMENT'] = selector
            for selection in ('spindle::spool::', 'overlap_tests::'):
                run('controls-' + selector + '-' + selection.replace(':', '_'),
                    ['cargo', 'test', '--offline', '--locked', '-p', 'fabric_o11y', '--lib',
                     selection, '--', '--test-threads=1'])
        env['FABRIC_ACK_ADVANCE_EXPERIMENT'] = '0'
        run('probe', ['cargo', 'test', '--offline', '--locked', '--release', '-p',
                      'fabric_o11y', '--lib', 'spool_ack_position_timing_probe', '--',
                      '--ignored', '--nocapture', '--test-threads=1'])
        probes = []
        for line in (OUT / 'probe.stdout').read_text().splitlines():
            start = line.find('{"ack_frame_reads"')
            if start >= 0:
                row = json.loads(line[start:])
                if row.get('probe') == 'spool_ack_position':
                    probes.append(row)
        if len(probes) != 18:
            raise RuntimeError('expected eighteen successful probe arms')
        comparisons = []
        for size in (16 * 1024, 256 * 1024, 896 * 1024):
            for pair in (1, 2, 3):
                rows = {r['reuse']: r for r in probes if r['body_bytes'] == size and r['pair'] == pair}
                baseline, candidate = rows[False], rows[True]
                comparisons.append({'body_bytes': size, 'pair': pair,
                    'ack_median_ratio': statistics.median(candidate['ack_wall_ns']) / statistics.median(baseline['ack_wall_ns']),
                    'loop_wall_ratio': candidate['loop_wall_ns'] / baseline['loop_wall_ns'],
                    'baseline_ack_reads': baseline['ack_frame_reads'], 'candidate_ack_reads': candidate['ack_frame_reads']})
        common.dump(OUT / 'probe-results.json', probes)
        common.dump(OUT / 'probe-comparison.json', comparisons)
        old = json.loads((OLD / 'freeze.json').read_text())
        if old['status'] != 'complete':
            raise RuntimeError('incomplete retained server freeze')
        common.dump(OUT / 'server-freeze-reference.json', old)
        bins = {}
        for selector in ('0', '1'):
            env['FABRIC_ACK_ADVANCE_EXPERIMENT'] = selector
            run('build-' + selector, ['cargo', 'build', '--offline', '--locked', '--release',
                                      '-p', 'fabric_o11y', '--example', 'coupled_overlap_node'])
            source = Path(os.environ['CARGO_TARGET_DIR']) / 'release/examples/coupled_overlap_node'
            archive = OUT / ('node-' + selector + '.gz')
            with source.open('rb') as src, archive.open('xb') as dst:
                with gzip.GzipFile(fileobj=dst, mode='wb', mtime=0, filename='') as zipped:
                    shutil.copyfileobj(src, zipped)
            info = {'archive_sha256': common.sha(archive), 'decoded_sha256': common.sha(source),
                    'decoded_bytes': source.stat().st_size}
            folder = work / ('bins-' + selector)
            folder.mkdir()
            expand(archive, info, folder / 'node')
            for name in ('server', 'dump'):
                info = old['binaries'][name]
                expand(OLD / info['archive'], info, folder / name)
            bins[selector] = folder
        common.dump(OUT / 'executables.json', executables)
        if source_hashes != {name: common.sha(ROOT / name) for name in SOURCES}:
            raise RuntimeError('source changed during freeze')
        parent = common.cgroups.delegate()
        for label, selector, observer in [('observer-loop', '0', 'loop'),
                                          ('baseline-final', '0', 'final'),
                                          ('candidate-final', '1', 'final'),
                                          ('baseline-final-repeat', '0', 'final')]:
            os.environ['FABRIC_O8_OBSERVER'] = observer
            group, _ = common.cgroups.subgroup(parent, label, 512 * 2**20, 448 * 2**20, 256, 3, children=True)
            result = common.trial(work / label, OUT / label, 'serial', bins[selector], group, deadline, 'backlog')
            events = [json.loads(line) for line in (OUT / label / 'node.stdout').read_text().splitlines() if line.startswith('{')]
            result.update(label=label, ack_selector=selector, observer=next(e for e in events if e['event'] == 'observer'))
            arms.append(result)
            common.dump(OUT / 'arms.json', arms)
            shutil.rmtree(work / label)
            evidence_cap()
        loop, baseline, candidate, repeat = arms
        common.dump(OUT / 'comparison.json', {
            'observer_cpu_ratio': baseline['node_cpu_seconds'] / loop['node_cpu_seconds'],
            'candidate_vs_baselines': [{
                'baseline': base['label'],
                'node_cpu_ratio': candidate['node_cpu_seconds'] / base['node_cpu_seconds'],
                'observation_ack_median_ratio': candidate['observation_to_ack']['median_ms'] / base['observation_to_ack']['median_ms']
            } for base in (baseline, repeat)],
            'h1_all_large_pairs_ack_reduction_ge_10pct': all(r['ack_median_ratio'] <= .9 for r in comparisons if r['body_bytes'] >= 256 * 1024),
            'h2_observer_cpu_reduction_ge_25pct': baseline['node_cpu_seconds'] <= .75 * loop['node_cpu_seconds'],
            'production_promotion': False, 'service_single_candidate_sample': True})
        evidence_cap()
        complete = True
    finally:
        # Exact retained executable references prevent redundant binary copies
        # from consuming the finite failure-evidence allocation.
        references = []
        for name, info in executables.items():
            path = Path(name)
            if not path.exists():
                continue
            archive = ROOT / info['archive']
            if common.sha(archive) != info['archive_sha256']:
                raise RuntimeError('cannot clean executable: retained archive changed')
            with gzip.open(archive, 'rb') as stream:
                decoded = stream.read()
            if path.read_bytes() != decoded or common.sha(path) != info['decoded_sha256']:
                raise RuntimeError('cannot clean executable: exact comparison failed')
            references.append(dict(info, removed_path=name, exact_readback=True))
        common.dump(OUT / 'executable-cleanup-references.json', references)
        for item in references:
            Path(item['removed_path']).unlink()
        if complete:
            shutil.rmtree(work)
        common.dump(OUT / 'cleanup.json', {'complete': complete, 'scratch_removed': not work.exists(),
                                         'work': str(work), 'unarchived_failure_retained': not complete})


if __name__ == '__main__':
    main()
