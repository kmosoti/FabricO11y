#!/usr/bin/env python3
"""Freeze run-cap heap predictions from 16MiB training cells before 64MiB runs."""
import argparse
import json
from pathlib import Path

import memory_census as mc


def main():
    mc.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--destination', type=Path, required=True)
    parser.add_argument('--protocol', type=Path, required=True)
    args = parser.parse_args()
    if args.destination.exists():
        raise ValueError('fresh frozen prediction destination required')
    if any((args.source / f'target-64-repeat-{repeat}').exists() for repeat in (0, 1)):
        raise ValueError('64MiB holdout already started; prospective freeze refused')
    sources = {}
    models = {}
    for shape in ('steady', 'adversarial'):
        points = []
        for repeat, seed in enumerate((2703204353, 2703204354)):
            job = args.source / f'target-16-repeat-{repeat}'
            complete = json.loads((job/'complete.json').read_text())
            if complete['exit_code'] != 0 or complete['pairs'] != 9:
                raise ValueError('training campaign incomplete')
            for cap in (8, 16, 32):
                path = job/f'run-{cap}-{shape}/pair.json'
                pair = json.loads(path.read_text())
                if not all(pair['gates'].values()) or pair['seed'] != seed or pair['target_mib'] != 16:
                    raise ValueError('training semantic/provenance failure')
                sources[str(path.resolve())] = mc.digest(path)
                points.append({'cap_mib': cap, 'seed': seed,
                               'peak_live_heap_bytes': pair['rows']['bounded']['probe']['peak_live_heap_bytes']})
        xmean = sum(p['cap_mib'] for p in points)/len(points)
        ymean = sum(p['peak_live_heap_bytes'] for p in points)/len(points)
        slope = sum((p['cap_mib']-xmean)*(p['peak_live_heap_bytes']-ymean) for p in points)/sum(
            (p['cap_mib']-xmean)**2 for p in points)
        intercept = ymean-slope*xmean
        predictions = {str(cap): intercept+slope*cap for cap in (8, 16, 32)}
        if any(value <= 0 for value in predictions.values()):
            raise ValueError('nonpositive fitted heap prediction')
        constants = {str(cap): sum(p['peak_live_heap_bytes'] for p in points if p['cap_mib'] == cap)/2
                     for cap in (8, 16, 32)}
        models[shape] = {'model_A_constant_per_cap': {'predicted_peak_bytes_by_cap': constants,
                         'training_max_relative_error': max(abs(p['peak_live_heap_bytes']-constants[str(p['cap_mib'])])/
                                                           p['peak_live_heap_bytes'] for p in points)},
                         'model_B_linear': {'intercept_bytes': intercept, 'slope_bytes_per_run_mib': slope,
                         'gamma_bytes_per_run_byte': slope/mc.MIB,
                         'training_max_relative_error': max(abs(p['peak_live_heap_bytes']-predictions[str(p['cap_mib'])])/
                                                           p['peak_live_heap_bytes'] for p in points),
                         'predicted_peak_bytes_by_cap': predictions},
                         'training_points': points, 'holdout_target_mib': 64,
                         'same_prediction_for_both_seeds': True}
    result = {'models': models, 'training_source_sha256': sources,
              'protocol_sha256': mc.digest(args.protocol), 'predictor_sha256': mc.digest(Path(__file__)),
              'decision': {'relative_error': 'abs(observed-predicted)/observed',
                           'maximum_relative_error_threshold': .05,
                           'all_required_cells': 'steady/adversarial x 8/16/32MiB caps x both seeds = 12 heldout cells',
                           'accepted_if': 'independently for model A and B, all 12 cells have unchanged semantic gates and relative error <=0.05',
                           'interrupted_or_failed': 'inconclusive, never accepted',
                           'refit_after_holdout': 'forbidden for this frozen model'},
              'bigrows': 'excluded from linear fit; cap16==32 training plateau suggests another dominant stage',
              'cpu_and_cost_prediction': 'unsupported; noisy training pairs are not fitted',
              'identifiability': 'per-table concurrent occupancy unidentified; gamma does not identify a unique causal memory owner',
              'qualification': False}
    args.destination.parent.mkdir(parents=True, exist_ok=True)
    mc.dump(args.destination, result)
    print(json.dumps({'prediction': str(args.destination), 'models': models}))


if __name__ == '__main__':
    main()
