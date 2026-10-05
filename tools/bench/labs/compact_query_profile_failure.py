#!/usr/bin/env python3
"""Losslessly factor repeated native answer bytes out of their wrapper frames."""
import gzip,json
from pathlib import Path
import sys
sys.path.insert(0,str(Path(__file__).resolve().parent))
import preserve_query_profile_failure as base
p=base.p;DATA=base.DATA

def main():
 p.require_limits()
 out=DATA/'memory/retry-02/full-failure';objects=out/'objects'
 manifest=json.loads((out/'manifest.json').read_text());old=[]
 for row in manifest['answers']:
  path=objects/row['object'];raw=gzip.open(path,'rb').read()
  prefix,answer=raw.split(b',"answer":',1);answer,suffix=answer.rsplit(b',"population":',1)
  prefix+=b',"answer":';suffix=b',"population":'+suffix
  name=p.retain_bytes(answer,objects,'.answer.json')
  assert prefix+gzip.open(objects/name,'rb').read()+suffix==raw
  row.update(object=name,prefix_hex=prefix.hex(),suffix_hex=suffix.hex())
  old.append(path)
 p.dump(out/'manifest-compacted.json',manifest)
 for path in set(old):path.unlink()
 (out/'manifest-compacted.json').replace(out/'manifest.json')
 p.check_space(out,DATA/'memory')
 row=next(r for r in manifest['answers'] if r['file']=='answer-segment-scan-broad.jsonl')
 answer=json.loads(gzip.open(objects/row['object'],'rb').read())
 assert len(answer['rows'])==10000 and answer['next_page'] is not None
 p.dump(out/'counterexample.json',{'origin':'allocation-retry-02','records':65536,'shape':'broad','limit':10000,
 'actual_first_page_rows':10000,'continuation_present':True,'violated_harness_assumption':'One first page is a complete pagination transcript.',
 'product_defect_established':False,'fix_not_applied':'Register a first-page checker or capture pagination outside measured spans.',
 'lossless_wrapper_roundtrips':len(manifest['answers'])})
 print(json.dumps({'memory_evidence_bytes':p.grader.footprint(DATA/'memory'),'exact_wrappers':len(manifest['answers'])}))
if __name__=='__main__':main()
