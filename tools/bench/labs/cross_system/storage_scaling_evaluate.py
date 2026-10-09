#!/usr/bin/env python3
"""Evaluate the frozen prospective heap models; never refit or widen thresholds."""
import argparse
import json
from pathlib import Path

import memory_census as mc

FROZEN_PREDICTION_SHA256 = 'e635a912d3170d1677f0863d252ce20e72405f545f7d374adaa235a9cab772c7'
MODEL_NAMES = ('model_A_constant_per_cap', 'model_B_linear')


def main():
    mc.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--destination', type=Path, required=True)
    args = parser.parse_args()
    if args.destination.exists() or not args.destination.resolve().is_relative_to(args.source.resolve()):
        raise ValueError('fresh evaluation inside source evidence root required')
    prediction_path = args.source/'scaling-predictions.json'
    if mc.digest(prediction_path) != FROZEN_PREDICTION_SHA256:
        raise ValueError('prospectively frozen prediction bytes changed')
    prediction = json.loads(prediction_path.read_text())
    if prediction['decision']['maximum_relative_error_threshold'] != .05:
        raise ValueError('frozen threshold is not 5%')
    for name, sha in prediction['training_source_sha256'].items():
        if mc.digest(Path(name)) != sha:
            raise ValueError('training receipt changed after freeze')
    coordinator = args.source.parent/'coordinator/storage-prediction'
    freeze_receipt = json.loads((coordinator/'receipt.json').read_text())
    emitted = json.loads((coordinator/'stdout.txt').read_text())
    if freeze_receipt['state'] != 'passed' or emitted['models'] != prediction['models']:
        raise ValueError('frozen models differ from completed prospective job stdout')
    models = {name: {'cells': [], 'maximum_relative_error': None,
                     'accepted': False, 'outcome': 'inconclusive'} for name in MODEL_NAMES}
    hashes = {}
    incomplete = []
    for repeat, seed in enumerate((2703204353, 2703204354)):
        job = args.source/f'target-64-repeat-{repeat}'
        if not (job/'complete.json').exists():
            incomplete.append(str(job))
            continue
        complete = json.loads((job/'complete.json').read_text())
        if complete['exit_code'] != 0 or complete['pairs'] != 9 or complete['native_cells'] != 18:
            incomplete.append(str(job))
            continue
        for shape in ('steady', 'adversarial'):
            for cap in (8, 16, 32):
                path = job/f'run-{cap}-{shape}/pair.json'
                pair = json.loads(path.read_text())
                hashes[str(path.resolve())] = mc.digest(path)
                if any(pair[key] != value for key, value in {'target_mib': 64, 'repeat': repeat,
                        'seed': seed, 'shape': shape, 'run_mib': cap}.items()):
                    raise ValueError('holdout identity mismatch')
                if not all(pair['gates'].values()):
                    incomplete.append(str(path))
                    continue
                actual = pair['rows']['bounded']['probe']['peak_live_heap_bytes']
                if actual <= 0:
                    raise ValueError('nonpositive heldout peak')
                for name in MODEL_NAMES:
                    wanted = prediction['models'][shape][name]['predicted_peak_bytes_by_cap'][str(cap)]
                    residual = actual-wanted
                    error = abs(residual)/actual
                    models[name]['cells'].append({'shape': shape, 'run_mib': cap,
                            'seed': seed, 'repeat': repeat, 'observed_bytes': actual,
                            'predicted_bytes': wanted, 'signed_residual_bytes': residual,
                            'relative_error': error, 'within_5_percent': error <= .05})
    for name, model in models.items():
        cells = model['cells']
        model['maximum_relative_error'] = max((row['relative_error'] for row in cells), default=None)
        if not incomplete and len(cells) == 12:
            model['accepted'] = all(row['within_5_percent'] for row in cells)
            model['outcome'] = 'accepted finite prediction' if model['accepted'] else 'falsified finite prediction'
        model['complete_cell_count'] = len(cells)
        model['by_shape_maximum_relative_error'] = {shape: max((row['relative_error'] for row in cells
                if row['shape'] == shape), default=None) for shape in ('steady', 'adversarial')}
    result = {'frozen_prediction_sha256': FROZEN_PREDICTION_SHA256, 'models': models,
              'incomplete_or_failed_holdouts': incomplete, 'holdout_receipt_sha256': hashes,
              'threshold': .05, 'refit_performed': False, 'qualification': False,
              'evaluator_sha256': mc.digest(Path(__file__)),
              'interpretation': 'model falsification alone is not a production memory defect; concurrent table occupancy remains unidentified'}
    mc.dump(args.destination, result)
    print(json.dumps({name: {key: value for key, value in model.items() if key != 'cells'}
                      for name, model in models.items()}))


if __name__ == '__main__':
    main()
