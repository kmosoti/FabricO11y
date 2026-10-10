"""Explain sampling-phase sensitivity without regrading a failed main cell."""
import argparse
import gzip
import json
from pathlib import Path

from release_timing import NS, native, p99


def diagnose(entries, begin, end):
    windows = [(begin, begin + 30 * NS), (end - 30 * NS, end)]
    means = []
    for left, right in windows:
        durations = [(max(0, min(e['ack_ns'], right) - max(e['created_ns'], left)), e['bytes'])
                     for e in entries]
        means.append({'count': sum(d for d, _ in durations) / (right-left),
                      'bytes': sum(d*b for d, b in durations) / (right-left)})
    phases = []
    for index in range(100):
        phase = index * NS // 20
        values = []
        for left, right in windows:
            samples = []
            for at in range(left + phase, right, 5 * NS):
                pending = [e for e in entries if e['created_ns'] <= at < e['ack_ns']]
                samples.append((len(pending), sum(e['bytes'] for e in pending)))
            values.append([sum(s[i] for s in samples)/len(samples) for i in (0, 1)])
        phases.append({'phase_ns': phase, 'first': values[0], 'last': values[1],
                       'original_inequality': all(b <= a for a,b in zip(*values))})
    latencies = [(e['ack_ns']-e['created_ns'])/NS for e in entries
                 if begin <= e['created_ns'] < end]
    # Queue-empty intervals distinguish persistently accumulated work from
    # short-lived in-flight occupancy. This is descriptive, not acceptance.
    intervals = sorted((max(begin, e['created_ns']), min(end, e['ack_ns'])) for e in entries
                       if e['created_ns'] < end and e['ack_ns'] > begin)
    merged = []
    for left, right in intervals:
        if merged and left <= merged[-1][1]:
            merged[-1][1] = max(merged[-1][1], right)
        else:
            merged.append([left, right])
    return {'classification': 'diagnostic only; original failure is unchanged',
            'continuous_first_last_mean': means, 'phases': phases,
            'phases_accepting_original_inequality': sum(p['original_inequality'] for p in phases),
            'phase_count': len(phases), 'ack_p99_s': p99(latencies), 'ack_max_s': max(latencies),
            'queue_empty_fraction': 1-sum(r-l for l,r in merged)/(end-begin),
            'max_continuous_nonempty_s': max((r-l)/NS for l,r in merged),
            'nonempty_intervals': len(merged)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('run', type=Path)
    args = parser.parse_args()
    summary = json.loads((args.run / 'summary.json').read_text())
    with gzip.open(args.run / 'edge-custody-spool.jsonl.gz', 'rt') as source:
        raw = [json.loads(line) for line in source if line.strip()]
    entries = native(args.run / 'edge.out', raw[:-1])
    samples = summary['timing']['edge']['samples']
    result = diagnose(entries, samples[0]['at_ns'], samples[-1]['at_ns'] + 5 * NS)
    (args.run / 'backlog-diagnostic.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({k:v for k,v in result.items() if k != 'phases'}))


if __name__ == '__main__':
    main()
