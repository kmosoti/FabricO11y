"""Consolidate all registered CQ1 slices without dropping missing trials."""
import argparse
import gzip
import json
from pathlib import Path
import sys
import statistics

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits


def read_json(path):
    if path.exists():
        return json.loads(path.read_text())
    with gzip.open(path.with_suffix(path.suffix + '.gz'), 'rt') as stream:
        return json.load(stream)


def equal(actual, expected, name):
    if actual != expected:
        raise RuntimeError(name + ' mismatch')


def main():
    require_limits()
    parser = argparse.ArgumentParser()
    parser.add_argument('--diagnostic', type=Path, required=True)
    parser.add_argument('--pairs', type=Path, nargs=3, required=True)
    args = parser.parse_args()
    # Representative completeness/provenance defects must not be accepted.
    for actual, expected in ((17, 18), ({'binary': 'changed'}, {'binary': 'fixed'}),
                             (('source', 'changed'), ('source', 'ranks'))):
        try:
            equal(actual, expected, 'negative control')
        except RuntimeError:
            pass
        else:
            raise RuntimeError('consolidation defect accepted')
    hashes = read_json(args.diagnostic / 'environment.json')['binary_hashes']
    identities, rows, comparisons, oracle_calls = {}, [], [], 0
    for index, directory in enumerate([args.diagnostic, *args.pairs]):
        environment = read_json(directory / 'environment.json')
        equal(environment['binary_hashes'], hashes, 'frozen binary set')
        cleanup = read_json(directory / 'cleanup.json')
        equal(cleanup['status'], 'completed', 'slice completion')
        trials = read_json(directory / 'trials.json')
        equal(len(trials), 9 if index == 0 else 3, 'slice size')
        if index:
            equal({r['pair'] for r in trials}, {index}, 'pair identity')
            equal({r['variant'] for r in trials}, {'plain'}, 'plain performance population')
            equal({r['mechanism'] for r in trials}, {'baseline', 'clone', 'shared'}, 'mechanisms')
        for trial in trials:
            evidence = directory / trial['evidence']
            fixture = read_json(evidence / 'fixture.json')['fixture']
            identity = tuple(fixture[k] for k in ('source_sha256', 'timestamp_ranks_sha256', 'encoded_batch_bytes'))
            count = fixture['records']
            equal(identity, identities.setdefault(count, identity), 'cross-slice fixture')
            verdicts = read_json(evidence / 'oracle.json')['verdicts']
            equal(len(verdicts), 64, 'full chain count')
            expected = {(f'answer-{kind}-{plan}-{shape}.jsonl', iteration)
                        for kind in ('tail', 'segment') for plan in ('scan', 'walk')
                        for shape in ('empty', 'selective', 'common', 'broad') for iteration in range(4)}
            equal({(v['file'], v['index']) for v in verdicts}, expected, 'unique full chain identities')
            for verdict in verdicts:
                chain = verdict['file'].replace('answer-', 'chain-', 1).replace('.jsonl', f"-{verdict['index']}.jsonl")
                equal(verdict['chain'], chain, 'first-page chain association')
            verification = read_json(evidence / 'archive-verification.json')
            equal((verification['wrappers'], verification['chains'], verification['exact_original_bytes_compared']),
                  (64, 64, True), 'exact archive coverage')
            equal(all(v['verdict']['passed'] for v in verdicts), True, 'oracle verdicts')
            oracle_calls += len(verdicts)
            with gzip.open(evidence / 'timings.jsonl.gz', 'rt') as stream:
                timings = [json.loads(line) for line in stream]
            warm = [r for r in timings if r['stage'] == 'query_tail_walk_broad_warm']
            equal(len(warm), 3, 'warm measurement count')
            actual = {key: statistics.median(r[key] for r in warm) for key in ('wall_ns', 'cpu_ns')}
            boundary = [r for r in timings if r['stage'] == 'measured_queries_complete']
            equal(len(boundary), 1, 'pre-continuation memory boundary')
            actual['measured_hwm_kib'] = boundary[0]['vm_hwm_kib']
            for key, value in actual.items():
                equal(trial['metrics'][key], value, 'raw timing ' + key)
        rows.extend(trials)
        recorded = read_json(directory / 'comparison.json')
        recomputed = []
        if index:
            selected = {r['mechanism']: r['metrics'] for r in trials}
            for candidate, baseline in (('clone', 'baseline'), ('shared', 'clone')):
                ratios = {key: selected[candidate][key] / selected[baseline][key]
                          for key in ('wall_ns', 'cpu_ns', 'measured_hwm_kib')}
                recomputed.append({'pair': index, 'baseline': baseline, 'candidate': candidate,
                    'ratios': ratios, 'cq1_guard_met': all(v <= 1.05 for v in ratios.values())
                    if candidate == 'clone' else None})
        equal(recorded, recomputed, 'independent paired arithmetic')
        comparisons.extend(recomputed)
    equal(len(rows), 18, 'complete matrix')
    equal(len(comparisons), 6, 'paired comparisons')
    print(json.dumps({'trials': len(rows), 'full_chain_oracle_calls': oracle_calls,
        'binary_hashes': hashes, 'comparisons': comparisons,
        'cq1_performance_guard_met': all(c['cq1_guard_met'] for c in comparisons if c['candidate'] == 'clone'),
        'scope': 'finite corrected catalog comparison; shared one-Segment observations are diagnostic'}, indent=2))


if __name__ == '__main__':
    main()
