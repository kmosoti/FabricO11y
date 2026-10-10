#!/usr/bin/env python3
"""Preserve the actual failed oracle input without regenerating native answers."""
import gzip
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import sys
ROOT=Path(__file__).resolve().parents[3]
spec=importlib.util.spec_from_file_location('profile_fixture',ROOT/'tools/bench/labs/query_compare/profile.py')
p=importlib.util.module_from_spec(spec);spec.loader.exec_module(p)
DATA=ROOT/'docs/experiments/benchmarks/data/query-plan-run-01'

def main():
    p.require_limits()
    source=p.STORAGE/'evidence/fabric-work-a9ef4f78d12d429ba8a5f1794300c6a5/memory/allocation-retry-02/tmp/query-allocation-profile/full-plain'
    out=DATA/'memory/retry-02/full-failure';out.mkdir(exist_ok=False)
    objects=out/'objects';objects.mkdir()
    ledgers={}
    for kind,rel in [('fixture','records.jsonl'),('tail','tail-records/records.jsonl'),('segment','segment-records/records.jsonl')]:
        ledgers[kind]=p.retain_ledger((source/rel).read_bytes(),objects)
    answers=[]
    for path in sorted(source.glob('answer-*.jsonl')):
        for i,line in enumerate(path.read_bytes().splitlines()):
            # Preserve complete native wrapper, not just the returned rows.
            answers.append({'file':path.name,'index':i,'object':p.retain_bytes(line,objects,'.answer-wrapper.json')})
    p.grader.compress(source/'timings.jsonl',out/'timings.jsonl.gz')
    p.dump(out/'manifest.json',{'ledgers':ledgers,'answers':answers,'source_sha256':p.grader.sha(source/'source.log'),
        'source_reconstruction':'Decode fixture Batch log bodies in order and append one newline per body.',
        'failure':'First-page probe was passed to complete-pagination oracle; full run is inconclusive.',
        'source_probe':'crates/fabric-server/examples/responsibility_probe.rs: query shapes limit=10000; one History.run per measured call',
        'grader':'tools/bench/run_responsibility_isolation.py:grade_query calls query_oracle.check with one page'})
    p.check_space(out,DATA/'memory')
    # Regenerate no answer: verify the retained first page is explicitly paginated.
    row=json.loads((source/'answer-segment-scan-broad.jsonl').read_bytes().splitlines()[0])
    assert len(row['answer']['rows'])==10000 and row['answer']['next_page'] is not None
    p.dump(out/'counterexample.json',{'origin':'allocation-retry-02','records':65536,'shape':'broad','limit':10000,
        'actual_first_page_rows':len(row['answer']['rows']),'continuation_present':True,
        'violated_harness_assumption':'One recorded first page is a complete pagination transcript.',
        'product_defect_established':False,'fix_not_applied':'Use a registered first-page checker or capture complete pagination outside measured spans.'})
    print(json.dumps({'preserved_answers':len(answers),'memory_evidence_bytes':p.grader.footprint(DATA/'memory')}))
if __name__=='__main__':main()
