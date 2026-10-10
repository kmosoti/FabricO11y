#!/usr/bin/env python3
"""Recompute phase accounting from preserved raw observations, without rerunning load."""
import argparse
import gzip
import json
from pathlib import Path
import observe_dev_small as observer


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root', type=Path)
    args = parser.parse_args()
    observer.require_limits()
    root = args.root
    summary = json.loads((root/'summary.json').read_text())
    samples = json.loads((root/'resources.json').read_text())
    with gzip.open(root/'recovered-hashes.jsonl.gz','rt') as stream:
        batches = [json.loads(s) for s in stream]
    sizes = {(r[0],r[1]):r[3] for r in batches}
    events = {}
    for path in root.glob('node*-events.jsonl.gz'):
        with gzip.open(path,'rt') as stream:
            events[path.name.split('-')[0]] = [json.loads(s) for s in stream]
    epoch = summary['observation']['epoch_ns']
    summary['observation']['rates'] = observer.phase_rates(epoch, samples[-1]['wall_ns'], sizes, events, root/'sources.jsonl.gz')
    summary['observation']['cpu'] = observer.cpu_windows(samples, epoch)
    summary['accounting_correction'] = 'Integer nanosecond phase boundaries; original summary and failed checker retained. Timed workload unchanged.'
    observer.native.dump(root/'summary.json', summary)
    print(json.dumps({'phase_source_logs':{k:v['counts'].get('source_logs',0) for k,v in summary['observation']['rates'].items()}}))


if __name__=='__main__':
    main()
