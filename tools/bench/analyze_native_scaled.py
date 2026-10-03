#!/usr/bin/env python3
"""Post-hoc dimensions and a separate counted-clock percentile cross-check."""
import argparse, gzip, json, math
from pathlib import Path


def analyze(root):
    summary=json.loads((root/'summary.json').read_text())
    samples=json.loads((root/'resources.json').read_text())
    epoch=summary['producer']['epoch_ns']
    active=[s for s in samples if epoch<=s['wall_ns']<=epoch+25_000_000_000]
    windows={}
    for label,window in [('scheduled_25s',active),('whole_monitored_phase',samples)]:
        first,last=window[0],window[-1]
        seconds=(last['mono_ns']-first['mono_ns'])/1e9
        windows[label]={'sample_count':len(window),'observed_seconds':seconds,
            'server_cpu_equivalents':(last['server']['cpu_s']-first['server']['cpu_s'])/seconds,
            'nodes_cpu_equivalents':(sum(n['cpu_s'] for n in last['nodes'])-sum(n['cpu_s'] for n in first['nodes']))/seconds}
    deltas={name:[] for name in ['ingest','ack','source_ingest','scheduled_ingest','source_ack','source_collection_wait']}
    with gzip.open(root/'clock-groups.jsonl.gz','rt') as f:
        for line in f:
            node,tick,collected,received,ack,after,scheduled,phase,count=json.loads(line)
            values={'ingest':received-collected,'source_ingest':received-after,
                'scheduled_ingest':received-scheduled,'source_collection_wait':collected-after}
            if ack is not None:values.update(ack=ack-collected,source_ack=ack-after)
            for name,value in values.items():deltas[name].append((value,count))
    clocks={}
    for name,values in deltas.items():
        bins={}
        for ns,n in values:bins[ns]=bins.get(ns,0)+n
        total=sum(bins.values()); ranks=[math.ceil(total/2),math.ceil(total*99/100)]
        seen=0;results=[]
        for ns,n in sorted(bins.items()):
            seen+=n
            while ranks and seen>=ranks[0]:results.append(ns/1e6);ranks.pop(0)
        clocks[name]={'samples':total,'p50':results[0],'p99':results[1],
            'max':max(bins)/1e6,'negative':sum(n for ns,n in bins.items() if ns<0)}
        if name!='source_collection_wait':assert clocks[name]==summary['latency_ms']['all'][name],name
    assert all(c['samples']==summary['expected_logs'] for c in clocks.values())
    recovered={}
    with gzip.open(root/'batch-hashes.jsonl.gz','rt') as f:
        for line in f:
            node,seq,sha,length,received=json.loads(line)
            assert (node,seq) not in recovered
            recovered[(node,seq)]=sha
    observed={}
    for path in root.glob('node*-events.jsonl.gz'):
        node=path.name.split('-events')[0]
        with gzip.open(path,'rt') as f:
            for line in f:
                e=json.loads(line)
                if e['kind']=='attempt' and e['status']=='ack':observed[(node,int(e['sequence']))]=e['sha256']
    assert observed==recovered
    return {'post_hoc_not_new_acceptance_gates':True,'cpu_windows':windows,'recomputed_clocks_ms':clocks,
        'exact_ack_hash_map_recheck':True,'ack_hash_map_size':len(observed),
        'peak_aggregate_reported_source_backlog_bytes':max(sum(p['file_backlog'] for p in s['progress']) for s in samples),
        'reported_source_backlog_at_last_scheduled_window_sample_bytes':sum(p['file_backlog'] for p in active[-1]['progress']),
        'peak_journal_disk_bytes':max(s['journal_bytes'] for s in samples),
        'shared_cgroup_memory_range_bytes':[min(s['cgroup_memory_current'] for s in samples),max(s['cgroup_memory_current'] for s in samples)],
        'server_rss_first_last_mib':[samples[0]['server']['rss_kib']/1024,samples[-1]['server']['rss_kib']/1024],
        'server_proc_io_delta':{k:samples[-1]['server_io'][k]-samples[0]['server_io'][k] for k in samples[0]['server_io']}}


if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('root',type=Path);a=p.parse_args()
    print(json.dumps(analyze(a.root),indent=2))
