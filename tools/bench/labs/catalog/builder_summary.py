"""Independent arithmetic over completed CR1 paired measurements."""
import json
from pathlib import Path
import statistics
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits


def main():
    require_limits()
    path = ROOT / 'docs/experiments/benchmarks/data/catalog-builder-spill-run-01'
    result = json.loads((path / 'result.json').read_text())
    pairs = result['pairs']
    if len(pairs) != 24:
        raise RuntimeError('expected 24 complete paired measurements')
    summary = {}
    for cell in sorted({p['cell'] for p in pairs}):
        rows = [p for p in pairs if p['cell'] == cell]
        summary[cell] = {}
        for counted, fields in (
            (False, ('build_seconds', 'phase_cpu_s')),
            (True, ('cumulative_requested_bytes', 'successful_alloc_realloc_calls',
                    'incremental_peak_heap_bytes')),
        ):
            selected = sorted((p for p in rows if p['counted'] == counted),
                              key=lambda p: p['repetition'])
            if [p['repetition'] for p in selected] != [1, 2, 3]:
                raise RuntimeError('incomplete cell')
            for field in fields:
                ratios = [p['candidate'][field] / p['baseline'][field] for p in selected]
                summary[cell][field] = {'ratios': ratios, 'median_ratio': statistics.median(ratios),
                    'baseline': [p['baseline'][field] for p in selected],
                    'candidate': [p['candidate'][field] for p in selected]}
        summary[cell]['missed_guards'] = [
            {'counted': p['counted'], 'pair': p['repetition'],
             'guards': [k for k, v in p['performance_guards'].items() if not v]}
            for p in rows if not all(p['performance_guards'].values())]
    print(json.dumps({'cells': summary, 'nomination_guards_passed': result['nomination_guards_passed'],
                      'scaling_within_10_percent': result['scaling_within_10_percent']}, indent=2))


if __name__ == '__main__':
    main()
