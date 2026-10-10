"""Read archived C2 reports; diagnose physical differences, never qualify them."""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits


def require(condition, message):
    if not condition:
        raise ValueError(message)


def integer(value):
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def validate_report(report):
    for table in ('logs', 'metrics'):
        count = report['logical_row_counts'][table]
        require(integer(count), 'invalid logical row count')
        groups = report['pruning'][table]['group_rows']
        require(groups and all(integer(n) and n > 0 for n in groups), 'invalid group counts')
        require(sum(groups) == count, 'group/logical row count mismatch')
        require(report['pruning'][table]['row_groups'] == len(groups), 'group count mismatch')
        for width in ('10', '60'):
            summary = report['pruning'][table]['windows'][width]
            require(integer(summary['matched_rows']) and integer(summary['read_rows']), 'invalid window totals')
            rows = summary['windows']
            previous = None
            for window in rows:
                require(all(integer(window[k]) for k in ('from_ns', 'to_ns', 'matched_rows', 'read_rows')), 'invalid window value')
                require(window['to_ns'] - window['from_ns'] == int(width) * 1_000_000_000, 'malformed window width')
                require(previous is None or window['from_ns'] > previous, 'window order/duplication')
                require(window['matched_rows'] <= window['read_rows'] <= count, 'invalid admitted/matching count')
                previous = window['from_ns']
            require(sum(w['matched_rows'] for w in rows) == summary['matched_rows'], 'matched total mismatch')
            require(sum(w['read_rows'] for w in rows) == summary['read_rows'], 'admitted total mismatch')


def compare(reference, bounded):
    for report in (reference, bounded):
        validate_report(report)
    for field in ('input_sha256', 'journal_bytes', 'logical_row_counts', 'ordered_row_ledgers'):
        require(reference[field] == bounded[field], 'changed ' + field)
    differences = []
    for table in ('logs', 'metrics'):
        for width in ('10', '60'):
            left = reference['pruning'][table]['windows'][width]['windows']
            right = bounded['pruning'][table]['windows'][width]['windows']
            require(len(left) == len(right), 'different window count')
            for a, b in zip(left, right):
                for key in ('from_ns', 'to_ns', 'matched_rows'):
                    require(a[key] == b[key], 'changed window ' + key)
                if a['read_rows'] != b['read_rows']:
                    differences.append({'table': table, 'width_s': int(width),
                        'from_ns': a['from_ns'], 'to_ns': a['to_ns'],
                        'matched_rows': a['matched_rows'],
                        'reference_footer_admitted_rows': a['read_rows'],
                        'bounded_footer_admitted_rows': b['read_rows']})
    return differences


def controls():
    window = {'from_ns': 0, 'to_ns': 10_000_000_000, 'matched_rows': 1, 'read_rows': 2}
    pruning = {'row_groups': 1, 'group_rows': [2], 'windows': {}}
    for width in ('10', '60'):
        row = dict(window, to_ns=int(width) * 1_000_000_000)
        pruning['windows'][width] = {'matched_rows': 1, 'read_rows': 2, 'windows': [row]}
    baseline = {'input_sha256': 'a' * 64, 'journal_bytes': 100,
        'logical_row_counts': {'logs': 2, 'metrics': 2},
        'ordered_row_ledgers': {'logs': 'b' * 64, 'metrics': 'c' * 64},
        'pruning': {'logs': copy.deepcopy(pruning), 'metrics': copy.deepcopy(pruning)}}
    compare(baseline, copy.deepcopy(baseline))
    rejected = []
    for defect in ('ordered_ledger', 'matched_rows', 'window_width', 'window_order'):
        mutant = copy.deepcopy(baseline)
        rows = mutant['pruning']['logs']['windows']['10']
        if defect == 'ordered_ledger':
            mutant['ordered_row_ledgers']['logs'] = 'd' * 64
        elif defect == 'matched_rows':
            rows['windows'][0]['matched_rows'] = 2
            rows['matched_rows'] = 2
        elif defect == 'window_width':
            rows['windows'][0]['to_ns'] += 1
        else:
            rows['windows'].append(dict(rows['windows'][0]))
            rows['matched_rows'] *= 2
            rows['read_rows'] *= 2
        try:
            compare(baseline, mutant)
        except ValueError:
            rejected.append(defect)
        else:
            raise RuntimeError('negative control accepted: ' + defect)
    extra = copy.deepcopy(baseline)
    for width in ('10', '60'):
        extra['pruning']['logs']['windows'][width]['windows'][0]['read_rows'] = 1
        extra['pruning']['logs']['windows'][width]['read_rows'] = 1
    require(len(compare(baseline, extra)) == 2, 'physical difference hidden')
    return {'representative_rejections': rejected,
            'physical_read_difference_reported_without_changing_existing_gate': True}


def load(path):
    raw = path.read_bytes()
    return json.loads(raw), hashlib.sha256(raw).hexdigest()


def main():
    require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--reports', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    require(not args.out.exists(), 'fresh output directory required')
    evidence, hashes = {}, {}
    for name in ('1-reference.readback.json', '1-bounded.readback.json', 'pair-1.json'):
        evidence[name], hashes[name] = load(args.reports / name)
    pair = evidence['pair-1.json']
    for variant in ('reference', 'bounded'):
        for field in ('input_sha256', 'journal_bytes', 'logical_row_counts', 'ordered_row_ledgers', 'pruning'):
            require(pair[variant][field] == evidence[f'1-{variant}.readback.json'][field], 'pair/readback evidence drift')
    differences = compare(evidence['1-reference.readback.json'], evidence['1-bounded.readback.json'])
    require(pair['gates']['pruning_equivalence'] == (not differences), 'recorded pruning gate inconsistent with archived windows')
    result = {'status': 'archived-report consistency checked; pruning soundness inconclusive',
        'input_report_sha256': hashes, 'controls': controls(), 'differences': differences,
        'historical_gates_unchanged': pair['gates'],
        'limitations': ['Footer-admitted rows are hypothetical counts, not actual IO or measured query visits.',
            'Rust producer suppresses admitted reads for windows with zero matching rows; this diagnostic preserves that evidence.',
            'Saved reports omit actual per-group min/max and row identity/source mapping.',
            'Whole-table ordered hashes are differential evidence, not independent complete paginated query answers.',
            'No omitted-group or narrowed-bound soundness claim can be made from these reports.',
            'Historical pruning-equivalence failure remains failed; no oracle/criterion is edited.']}
    args.out.mkdir(parents=True)
    (args.out / 'diagnostic.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
