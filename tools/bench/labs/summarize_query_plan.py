#!/usr/bin/env python3
"""Report the registered paired comparison without changing acceptance gates."""
import json
from pathlib import Path
import statistics
import sys
sys.path.insert(0, str(Path(__file__).resolve().parent))
import summarize_dev_small as old
DATA = old.ROOT/'docs/experiments/benchmarks/data/query-plan-run-01'
old.DATA = DATA

def main():
    old.require_limits()
    cells = {}
    for path in sorted((DATA/'query').glob('*')):
        if not (path/'independent-audit.json').exists():
            continue
        c = old.cell(path)
        s = old.load(path/'summary.json')
        a = old.load(path/'independent-audit.json')
        c['demand'] = a.get('query_workload')
        c['peak_file_backlog_bytes'] = s['observation']['peak_file_backlog_bytes']
        c['final_backlog_zero'] = all(not p['pending_batches'] and not p['file_backlog_bytes'] for p in s['observation']['progress_final'])
        c['cpu_seconds_per_completed_request_including_ingestion'] = c['server_timed_cpu_seconds']/236
        cells[path.name] = c
    pairs = []
    for n in (1,2,3):
        names = [f'small-{plan}-{n}' for plan in ('scan','walk')]
        if not all(x in cells for x in names):
            continue
        a,b = [cells[x] for x in names]
        pairs.append({'pair':n, 'eligible':all(c['passed'] and c['timing_valid'] and c['independent_audit_passed'] for c in (a,b)),
            **{k:b[k]/a[k] for k in ('balanced_server_cpu_cores','phase_peak_server_rss_mib','ack_observed_p99_ms')}})
    decision = {'eligible':len(pairs)==3 and all(p['eligible'] for p in pairs)}
    if decision['eligible']:
        med = lambda key: statistics.median(p[key] for p in pairs)
        scans = [cells[f'small-scan-{n}'] for n in (1,2,3)]
        walks = [cells[f'small-walk-{n}'] for n in (1,2,3)]
        guards = {'cpu':all(p['balanced_server_cpu_cores']<1 for p in pairs) and med('balanced_server_cpu_cores')<=.85,
            'rss':med('phase_peak_server_rss_mib')<=1.10,
            'ack':statistics.median(c['ack_observed_p99_ms'] for c in walks)<=1.05*statistics.median(c['ack_observed_p99_ms'] for c in scans),
            'final_backlog_and_retries':all(c['final_backlog_zero'] and not c['retries'] for c in scans+walks)}
        for k in ('peak_pending_batches','peak_file_backlog_bytes'):
            guards[k]=statistics.median(c[k] for c in walks)<=statistics.median(c[k] for c in scans)
        latency={}
        for shape in ('recent_logs','absent_text','cpu_metrics'):
            for phase in old.PHASES:
                a,b=[statistics.median(c['queries'][shape][phase]['latency_ms']['p99'] for c in group) for group in (scans,walks)]
                key=shape+':'+phase
                latency[key]={'scan_median_p99_ms':a,'walk_median_p99_ms':b,'limit_ms':max(1.1*a,a+2)}
                guards['latency:'+key]=b<=max(1.1*a,a+2)
        decision.update(guards=guards, favorable=all(guards.values()), latency=latency,
            median_cpu_ratio=med('balanced_server_cpu_cores'),median_rss_ratio=med('phase_peak_server_rss_mib'))
    result={'cells':cells,'pairs':pairs,'decision':decision,'latency_interpretation':'Conservative p99 for each shape and offered phase; 20 requests per cell/shape/phase. No significance or deployment qualification.'}
    with (DATA/'consolidated.json').open('x') as stream:
        json.dump(result,stream,indent=2);stream.write('\n')
    print(json.dumps({'cells':list(cells),'decision':decision}))
if __name__=='__main__':main()
