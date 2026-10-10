#!/usr/bin/env python3
"""Rejecting controls for the native pilot audit and post-hoc clock checker.

Origin: native-scaled-run-01 enterprise monitor stopped on concurrent Spool
append-in-progress publication; run_native_scaled.controls minimizes that race.
The other fixtures inject an altered summary percentile and recovered ACK hash.
"""
import argparse, gzip, json, shutil, tempfile
from pathlib import Path
from analyze_native_scaled import analyze
from run_native_scaled import controls


def check(root):
    results={'runner_controls':controls(),'clock_mutation_rejected':False,'ack_mutation_rejected':False}
    with tempfile.TemporaryDirectory(prefix='native-clock-controls-') as tmp:
        p=Path(tmp)/'medium';shutil.copytree(root/'medium',p)
        original=(p/'summary.json').read_text();s=json.loads(original)
        s['latency_ms']['all']['ingest']['p99']+=1
        (p/'summary.json').write_text(json.dumps(s))
        try:analyze(p)
        except AssertionError:results['clock_mutation_rejected']=True
        (p/'summary.json').write_text(original)
        f=p/'batch-hashes.jsonl.gz'
        with gzip.open(f,'rt') as src:lines=src.readlines()
        row=json.loads(lines[0]);row[2]='0'*64;lines[0]=json.dumps(row)+'\n'
        with gzip.open(f,'wt') as out:out.writelines(lines)
        try:analyze(p)
        except AssertionError:results['ack_mutation_rejected']=True
    assert results['clock_mutation_rejected'] and results['ack_mutation_rejected']
    return results


if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('root',type=Path);a=p.parse_args()
    print(json.dumps(check(a.root),indent=2))
