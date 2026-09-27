#!/usr/bin/env python3
"""Derive descriptive tables from a complete preserved run; no timing runs here."""
import json, statistics, sys
from pathlib import Path
p=Path(sys.argv[1]);out=Path(sys.argv[2])
meta=json.loads((p/'metadata.json').read_text())
assert meta['status']=='complete' and meta['mode']=='formal'
summary=json.loads((p/'summary.json').read_text())
med=statistics.median

def rank(v,n):return sorted(v)[(len(v)*n+99)//100-1]
def timing(rows,key):
 return {f'median_{unit}':med(r[key][unit] for r in rows) for unit in ['wall_ns','cpu_ns']}
def resources(rows):
 return {'median_rss_kib':med(r['rss_kib'] for r in rows),'median_process_cpu_ns':med(r['process_cpu_ns'] for r in rows),'kernel_io_medians':{k:med(r['kernel_io_delta'][k] for r in rows) for k in ['rchar','wchar','read_bytes','write_bytes']},'median_timer_wall_ns':med(x['wall_ns'] for r in rows for x in r['calibration']),'median_timer_cpu_ns':med(x['cpu_ns'] for r in rows for x in r['calibration'])}
def qstats(rows,mode):
 return {f'median_trial_p{pct}_{unit}':med(rank([s['timing'][unit] for s in r['samples'] if s['mode']==mode],pct) for r in rows) for pct in [50,99] for unit in ['wall_ns','cpu_ns']}
result={}
for key,cell in summary.items():
 layouts={}
 jr=[json.loads((p/f'{key}-{t}-json64/result.json').read_text()) for t in range(5)]
 for name in ['json64','plain64','zstd64','zstd256']:
  rows=[json.loads((p/f'{key}-{t}-{name}/result.json').read_text()) for t in range(5)]
  d={'publication':timing(rows,'publication'),'file_read':timing(rows,'file_read'),'resources':resources(rows),'queries':{m:qstats(rows,m) for m in ['full','projected']+([] if name=='json64' else ['postings'])},'family_paired_projection_wall_ratios':{}}
  for f in range(8):
   ratios=[]
   for r,j in zip(rows,jr):
    base={s['index']:s['timing']['wall_ns'] for s in j['samples'] if s['mode']=='projected'}
    ratios += [s['timing']['wall_ns']/base[s['index']] for s in r['samples'] if s['mode']=='projected' and s['family']==f]
   d['family_paired_projection_wall_ratios'][str(f)]={'median':med(ratios),'p99':rank(ratios,99)}
  if name!='json64':d.update(index_build=timing(rows,'index_build'),index_publication=timing(rows,'index_publication'))
  layouts[name]=d
 result[key]={'layouts':layouts}
 if 'receipt' in cell:
  rows=[json.loads((p/f'{key}-{t}-receipt.json').read_text()) for t in range(5)]
  result[key]['receipt']={'phases':{x:timing(rows,x) for x in ['scalar_build','sealed_build','verify','partial_resume_merge','replay']},'queries':{m:qstats(rows,m) for m in ['scalar','receipt']},'resources':resources(rows),'bytes':{x:med(r[x] for r in rows) for x in ['full_page_bytes','two_page_bytes','residual_bytes']},'family_median_page_bytes':{str(f):med(s['page_bytes'] for r in rows for s in r['samples'] if s['mode']=='receipt' and s['family']==f) for f in range(8)}}
  result[key]['sidecar']={}
  for role in ['source-cost','sidecar-baseline','sidecar-valid','sidecar-absent','sidecar-corrupt','sidecar-stale','sidecar-underinclusive']:
   rs=[json.loads((p/f'{key}-{t}-{role}.json').read_text()) for t in range(5)]
   result[key]['sidecar'][role]={'phase':timing(rs,'phase'),'resources':resources(rs),'fallbacks':[r.get('fallback') for r in rs]}
out.write_text(json.dumps({'definition':'Phase/resource medians across five measured trials; query values are medians of each trial nearest-rank p50/p99. Family ratios pair matching query indices across five trials; family page bytes are pooled within each fixed dataset/family. No cross-dataset gate pooling.','cells':result},indent=2)+'\n')
print('derived descriptive metrics for',len(result),'datasets')
