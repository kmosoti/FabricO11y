"""Derive descriptive tables without changing any recorded decision rule."""
import csv,gzip,json,math,os,statistics,sys
from collections import defaultdict
from pathlib import Path
ROOT=Path(__file__).resolve().parents[4]
sys.path.insert(0,str(ROOT/'tools'))
from resource_group import require_limits
DATA=ROOT/'docs/experiments/benchmarks/data/lab-completion-run-01'
def read(p):return json.loads(p.read_text())
def lines(p):
 with gzip.open(p,'rt') as f:return [json.loads(x) for x in f]
def write(p,r):p.write_text(json.dumps(r,indent=2)+'\n')
def table(p,rows):
 if not rows:return
 with p.open('w') as f:
  out=csv.DictWriter(f,fieldnames=list(rows[0]));out.writeheader();out.writerows(rows)
def pct(rows):
 if not rows:return None
 rows=sorted(rows);return {'n':len(rows),'median':statistics.median(rows),'p99':rows[math.ceil(.99*len(rows))-1],'max':max(rows)}
def main():
 require_limits();builder=[];query=[];resources=[];signals=[]
 for cell in sorted((DATA/'memory').glob('builder-*')):
  pairs=[read(p) for p in sorted(cell.glob('pair-*.json'))]
  if not pairs:continue
  builder.append({'cell':cell.name,'pairs':len(pairs),'implemented_gates':all(all(p['gates'].values()) for p in pairs),
   'reference_heap_mib':max(p['reference']['incremental_peak_heap_bytes'] for p in pairs)/2**20,
   'bounded_heap_mib':max(p['bounded']['incremental_peak_heap_bytes'] for p in pairs)/2**20,
   'median_heap_ratio':statistics.median(p['bounded']['incremental_peak_heap_bytes']/p['reference']['incremental_peak_heap_bytes'] for p in pairs),
   'reference_build_seconds':statistics.median(p['reference']['build_seconds'] for p in pairs),
   'bounded_build_seconds':statistics.median(p['bounded']['build_seconds'] for p in pairs),
   'reference_cpu_seconds':statistics.median(p['reference']['build_cpu_user_s']+p['reference']['build_cpu_system_s'] for p in pairs),
   'bounded_cpu_seconds':statistics.median(p['bounded']['build_cpu_user_s']+p['bounded']['build_cpu_system_s'] for p in pairs)})
 for variant in ('plain','counted'):
  path=DATA/'query/profile-01'/('full-'+variant)/'timings.jsonl.gz'
  if not path.exists():continue
  groups=defaultdict(list)
  for r in lines(path):
   if r['stage'].startswith('query_') and 'wall_ns' in r:groups[r['stage']].append(r)
  for stage,rows in groups.items():
   query.append({'variant':variant,'stage':stage,'n':len(rows),'median_wall_ms':statistics.median(r['wall_ns'] for r in rows)/1e6,
    'median_cpu_ms':statistics.median(r['cpu_ns'] for r in rows)/1e6,
    'median_incremental_peak_mib':statistics.median(r['allocation']['incremental_peak_bytes'] for r in rows)/2**20 if variant=='counted' else None,
    'median_allocated_mib':statistics.median(r['allocation']['cumulative_requested_bytes'] for r in rows)/2**20 if variant=='counted' else None})
 for job in sorted((DATA/'coordinator').glob('*/receipt.json')):
  r=read(job)
  if r['state']=='running':continue
  final=r['cgroup_final'];event=dict(line.split() for line in final['memory.events'].splitlines())
  cpu=dict(line.split() for line in final['cpu.stat'].splitlines())
  resources.append({'job':r['id'],'state':r['state'],'exit':r['exit'],'elapsed_s':r['elapsed_s'],
   'cgroup_peak_mib':int(final['memory.peak'])/2**20,'oom':int(event['oom']),'oom_kill':int(event['oom_kill']),
   'swap_bytes':int(final['memory.swap.current']),'cpu_s':int(cpu['usage_usec'])/1e6,
   'sampled_scratch_mib':r['peak_sampled_scratch_bytes']/2**20})
  path=job.parent/'resources.jsonl.gz'
  if path.exists():
   samples=lines(path);pids={};concurrent=defaultdict(float)
   for sample in samples:
    by_role=defaultdict(int)
    for pid,p in sample['processes'].items():
     pids[pid]=p;by_role[p['name']]+=p['rss_kib']
    for role,n in by_role.items():concurrent[role]=max(concurrent[role],n/1024)
   cpus=defaultdict(float)
   for p in pids.values():cpus[p['name']]+=(p['user_ticks']+p['system_ticks'])/os.sysconf('SC_CLK_TCK')
   write(job.parent/'component-observations.json',{'last_observed_lifetime_cpu_seconds_by_name':dict(cpus),
       'peak_sampled_concurrent_rss_mib_by_name':dict(concurrent),
       'limitation':'5second sampling misses terminal CPU increments and short peaks; same-name processes include restarts; no exclusive phase attribution'})
 for lab in ('memory','recovery'):
  for cell in (DATA/lab).iterdir():
   if not cell.is_dir() or not (cell/'events.jsonl.gz').exists():continue
   summary=read(cell/'summary.json');events=lines(cell/'events.jsonl.gz');buckets=defaultdict(lambda:defaultdict(int));seen=set()
   for e in events:
    b=buckets[e['wall_ns']//1_000_000_000]
    if e['kind']=='cycle':b['successful_collection_batches']+=1;b['collection_logs']+=int(e['logs']);b['collection_metrics']+=int(e['metrics'])
    else:
     b['delivery_attempts']+=1;b['delivery_'+e['status']]+=1
     key=(e['label'],e['sequence'])
     if e['status']=='ack' and key not in seen:b['unique_server_acks']+=1;seen.add(key)
   sdk=cell/'sdk/attempts.jsonl'
   if sdk.exists():
    for line in sdk.read_text().splitlines():
     e=json.loads(line);b=buckets[e['end_wall_ns']//1_000_000_000];b['sdk_attempts']+=1
     if e['response_status']==200:b['sdk_local_accepts']+=1;b['sdk_local_accepted_bytes']+=e['body_bytes']
   write(cell/'acceptance-per-second.json',{'buckets':dict(buckets),
      'distinction':'successful collection-cycle batches omit trace-only commits; SDK local responses count exports, server ACKs count Batches; do not add incompatible denominators'})
   by_kind=defaultdict(list)
   for q in lines(cell/'queries.jsonl.gz'):
    if 'elapsed_ms' in q:by_kind[q['query']['kind']].append(q['elapsed_ms'])
   write(cell/'query-latencies-by-kind.json',{kind:pct(vals) for kind,vals in by_kind.items()})
   clocks=cell/'span-clocks.jsonl.gz';latencies={}
   if clocks.exists():
    rows=lines(clocks)
    for label,base in [('ack_to_observed_query_ms','server_ack_observed_ns'),('local_response_to_observed_query_ms','local_spool_response_ns')]:
     latencies[label]=pct([(x['first_query_observed_ns']-x[base])/1e6 for x in rows if x.get('first_query_observed_ns') is not None and x.get(base) is not None])
    latencies['spans_without_query_observation']=sum(x['first_query_observed_ns'] is None for x in rows)
    write(cell/'visibility-observation-summary.json',latencies)
   signals.append({'cell':cell.name,'passed':summary['passed'],'logs':summary['logs'],'spans':summary['spans'],
       'batches':summary['batches'],'acks':summary['acks'],'sdk_local_response_p99_ms':summary['sdk_local_response_ms']['p99'],
       'query_p99_ms':summary['query_latency_ms']['p99'],'query_errors':len(summary['query_errors'])})
 table(DATA/'builder-summary.csv',builder);table(DATA/'query-allocation-summary.csv',query)
 table(DATA/'resource-summary.csv',resources);table(DATA/'signal-summary.csv',signals)
 print(json.dumps({'builder_cells':len(builder),'query_populations':len(query),'completed_jobs':len(resources),'signal_cells':len(signals)}))
if __name__=='__main__':main()
