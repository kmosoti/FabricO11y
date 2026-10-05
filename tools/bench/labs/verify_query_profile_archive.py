#!/usr/bin/env python3
"""Verify lossless profiling failure artifacts against original native files."""
import gzip,hashlib,json
from pathlib import Path
import sys
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT/'tools'))
from resource_group import require_limits,STORAGE
DATA=ROOT/'docs/experiments/benchmarks/data/query-plan-run-01'

def main():
 require_limits()
 (DATA/'recovery').mkdir(exist_ok=True)
 out=DATA/'memory/retry-02/full-failure';objects=out/'objects'
 source=STORAGE/'evidence/fabric-work-a9ef4f78d12d429ba8a5f1794300c6a5/memory/allocation-retry-02/tmp/query-allocation-profile/full-plain'
 manifest=json.loads((out/'manifest.json').read_text());verified=[]
 for row in manifest['answers']:
  got=bytes.fromhex(row['prefix_hex'])+gzip.open(objects/row['object'],'rb').read()+bytes.fromhex(row['suffix_hex'])
  expected=(source/row['file']).read_bytes().splitlines()[row['index']]
  assert got==expected
  verified.append({'file':row['file'],'index':row['index'],'raw_sha256':hashlib.sha256(got).hexdigest()})
 ledgers={}
 for kind,object_name in manifest['ledgers'].items():
  index=json.loads(gzip.open(objects/object_name,'rb').read())
  got=b''.join(bytes.fromhex(r['prefix_hex'])+gzip.open(objects/r['object'],'rb').read()+bytes.fromhex(r['suffix_hex']) for r in index['frames'])
  expected=(source/('records.jsonl' if kind=='fixture' else kind+'-records/records.jsonl')).read_bytes()
  assert got==expected and hashlib.sha256(got).hexdigest()==index['raw_sha256']
  ledgers[kind]=index['raw_sha256']
 report={'passed':True,'actual_wrapper_count':len(verified),'wrappers':verified,'ledgers':ledgers,
 'interpretation':'Lossless preservation only; the failed complete-pagination oracle verdict remains failed.'}
 with (DATA/'recovery/profile-archive-verification.json').open('x') as stream:json.dump(report,stream,indent=2)
 print(json.dumps({'passed':True,'wrappers':len(verified),'ledgers':len(ledgers)}))
if __name__=='__main__':main()
