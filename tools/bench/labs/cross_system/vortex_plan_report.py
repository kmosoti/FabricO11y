#!/usr/bin/env python3
"""Independent source-answer and paired-cost reduction of prepared-scan evidence."""
import argparse
import json
import math
from pathlib import Path
import statistics

import query_sweep_report as r

ARMS = ('new-scan', 'prepared-scan')


def coverage(rows):
    expected = {(arm, name, i) for arm in ARMS for name in r.NAMES for i in range(4)}
    seen = set()
    for row in rows:
        name = row['query']['name'] if isinstance(row['query'], dict) else row['query']
        key = row['arm'], name, row['iteration']
        if key not in expected or key in seen:
            raise RuntimeError('duplicate/unregistered prepared-scan measurement')
        seen.add(key)
    if seen != expected:
        raise RuntimeError('prepared-scan coverage differs from48 answers')


def controls():
    rows = [dict(arm=a, query=n, iteration=i) for a in ARMS for n in r.NAMES for i in range(4)]
    coverage(rows)
    rejected = []
    for name, bad in (('missing', rows[:-1]), ('duplicate', rows + rows[:1]),
                      ('wrong_arm', [dict(rows[0], arm='invalid'), *rows[1:]])):
        try:
            coverage(bad)
        except RuntimeError:
            rejected.append(name)
        else:
            raise RuntimeError('independent report coverage defect accepted')
    try:
        r.digest_check(b'valie', r.hashlib.sha256(b'valid').hexdigest(), 5)
    except RuntimeError:
        rejected.append('changed_answer')
    else:
        raise RuntimeError('independent report answer defect accepted')
    return rejected


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--inputs', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    r.require_limits()
    root = args.inputs.resolve()
    if args.out.exists():
        raise RuntimeError('fresh report path required')
    cleanup, env, inventory = (r.read(root / name) for name in ('cleanup.json', 'environment.json', 'results.json'))
    if cleanup['status'] != 'passed' or not cleanup['removed'] or len(inventory) != 8:
        raise RuntimeError('successful eight-child cleanup required')
    if not 1 <= len(env['cpu_affinity']) <= 2:
        raise RuntimeError('two-CPU affinity receipt required')
    pins = {pin[0]: pin[1] for pin in env['wheel_pins']}
    for name, version, filename, base, digest, size in env['wheel_pins']:
        receipt = r.read(root / (name + '-wheel.json'))
        if not receipt['verified'] or receipt['sha256'] != digest or receipt['bytes'] != size:
            raise RuntimeError('wheel provenance mismatch')
    for name, receipt in r.read(root / 'wheel-api-receipts.json').items():
        r.digest_check((root / 'wheel-api' / name).read_bytes(), receipt['sha256'], receipt['bytes'])
    checked = controls()
    sources, answers, seen, populations, paired, children = {}, {}, set(), [], [], []
    for item in inventory:
        cell = item['cell']
        key = cell['width'], cell['locality'], cell['repeat']
        if key in seen or cell['records'] != 32768 or cell['seed'] != 42 or cell['row_group'] != 8192:
            raise RuntimeError('duplicate/unregistered fixture')
        seen.add(key)
        evidence = root / f'b{cell["width"]}-{cell["locality"]}-r{cell["repeat"]}'
        result = r.read(evidence / 'result.json')
        if r.sha(evidence / 'result.json') != item['result_sha256'] or result['versions'] != pins or result['cell'] != cell:
            raise RuntimeError('worker result/provenance differs')
        for field in ('source', 'vortex', 'wildcard'):
            p = Path(cell[field])
            if p.is_symlink() or not p.is_file() or r.sha(p) != cell[field + '_sha256']:
                raise RuntimeError('historical input changed')
        digest = cell['source_sha256']
        if digest not in sources:
            sources[digest] = [json.loads(line) for line in r.decompress(Path(cell['source'])).splitlines()]
        logical = sources[digest]
        if len(logical) != 32768 or any(len(row) != 8 or len(row[6].encode()) != cell['width'] for row in logical):
            raise RuntimeError('source schema/count/width mismatch')
        measurements, verdicts = r.read(evidence / 'measurements.json'), r.read(evidence / 'verdicts.json')
        coverage(measurements)
        coverage(verdicts)
        objects = r.read(evidence / 'answer-objects.json')
        by_verdict = {(v['arm'], v['query'], v['iteration']): v for v in verdicts}
        for m in measurements:
            name = m['query']['name']
            expected, matches = r.source_expectation(logical, r.expected_query(name))
            v = by_verdict[m['arm'], name, m['iteration']]
            if m['query'] != r.expected_query(name) or m['matching_rows'] != matches or not v['passed']:
                raise RuntimeError('timed query semantics/verdict drift')
            digest = v['answer_raw_sha256']
            compressed = r.object_path(root / 'objects', objects[digest])
            if digest not in answers:
                raw = r.decompress(compressed, 2 * 1024**2)
                r.digest_check(raw, digest, v['answer_raw_bytes'])
                answers[digest] = json.loads(raw)
            if answers[digest] != expected:
                raise RuntimeError('retained answer differs from independently filtered source')
        wildcard = r.read(evidence / 'wildcard-control.json')
        a, b = logical[:2]
        bad_expected = [a[:6] + ['C%z', a[7]], b[:6] + ['Cxxz', b[7]]]
        if (not wildcard['rejected'] or wildcard['bad'] != bad_expected or wildcard['expected'] != bad_expected[:1]
                or wildcard['recovered'] != bad_expected[:1]):
            raise RuntimeError('actual wildcard defect/fallback not independently confirmed')
        if len(result['checker_controls']['rejected']) != 6 or not result['checker_controls']['valid_literal_control']:
            raise RuntimeError('answer checker controls incomplete')
        literals = result['literal_controls']
        if len(literals) != 14 or not all(c['passed'] for c in literals):
            raise RuntimeError('literal controls incomplete')
        process = r.read(evidence / 'process.json')
        if process['exit'] != 0:
            raise RuntimeError('worker receipt failed')
        children.append(dict(cell=cell, process=process, open=result['open'], preparation=result['preparation']))
        for name in r.NAMES:
            for arm in ARMS:
                for boundary, iterations in (('first', (0,)), ('reuse_median', (1, 2, 3))):
                    rows = [m for m in measurements if m['arm'] == arm and m['query']['name'] == name
                            and m['iteration'] in iterations]
                    populations.append(dict(cell=cell, arm=arm, query=name, boundary=boundary,
                        **{metric: statistics.median(m[metric] for m in rows) for metric in r.METRICS},
                        phase_medians=r.median_phases(rows)))
            models = [m for m in result['models'] if m['query'] == name]
            if len(models) != 2:
                raise RuntimeError('repayment coverage mismatch')
            for boundary in ('first', 'reuse_median'):
                a, b = [next(p for p in populations if p['cell'] == cell and p['query'] == name
                    and p['boundary'] == boundary and p['arm'] == arm) for arm in ARMS]
                metrics = {}
                for metric, phase_metric in zip(r.METRICS, ('wall_ns', 'process_cpu_ns')):
                    saving = a[metric] - b[metric]
                    cost = result['preparation'][name][phase_metric]
                    repay = math.ceil(cost / saving) if saving > 0 else None
                    if boundary == 'reuse_median':
                        model = next(m for m in models if m['metric'] == metric)
                        if any(model[k] != value for k, value in dict(baseline_median_ns=a[metric],
                            prepared_median_ns=b[metric], preparation_ns=cost, saving_ns=saving, queries_to_repay=repay).items()):
                            raise RuntimeError('recorded repayment differs from independent reduction')
                    metrics[metric] = dict(baseline_ns=a[metric], prepared_ns=b[metric],
                        ratio=b[metric] / a[metric], saving_ns=saving, preparation_ns=cost, queries_to_repay=repay)
                paired.append(dict(cell=cell, query=name, boundary=boundary, metrics=metrics))
    if seen != {(w, loc, rep) for w in (16, 1024) for loc in ('clustered', 'mixed') for rep in (0, 1)}:
        raise RuntimeError('fixture inventory mismatch')
    report = dict(exact_source_answers_rechecked=384, children=children, populations=populations, paired=paired,
        report_rejected_controls=checked, checker_rejections=48, actual_wildcard_rejections=8,
        engine_literal_checks=112, provenance=r.archive_provenance(root), environment=env, cleanup=cleanup,
        limitations=['Separate fresh-repetition medians, no pooled acceptance or OS cache claim.',
            'Shared process holds six prepared plans; no isolated arm memory or planning-only attribution.',
            'Preparation repayment is fixed-query arithmetic, not refill/update or native Fabric performance.'])
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, indent=2, ensure_ascii=False) + '\n')
    print(json.dumps(dict(report=str(args.out), exact_answers=384, children=8)))


if __name__ == '__main__':
    main()
