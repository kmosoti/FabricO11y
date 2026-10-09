"""Summarize recorded service/recovery observations; does not re-grade oracles."""
import hashlib
import json
from pathlib import Path
import statistics
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT/'tools'))
from resource_group import require_limits
BASE = ROOT/'docs/experiments/benchmarks/data/native-frontier-01'


def read(path):
    return json.loads(path.read_text())


def main():
    require_limits()
    out = BASE/'memory/service-recovery/results'
    out.mkdir(exist_ok=False)
    results = {}
    sources = {}
    for plan in ('scan', 'walk'):
        directory = BASE/'memory'/f'readiness-{plan}-01'
        summary = read(directory/'summary.json')
        resources = read(directory/'resources.json')
        observation = summary['observation']
        epoch = observation['epoch_ns']
        groups = {}
        for group in ('server', 'nodes'):
            windows = {}
            for name, begin, end in [('first_quiet', 180, 240), ('last_quiet', 300, 360)]:
                rows = [r for r in resources if begin <= (r['wall_ns']-epoch)/1e9 < end]
                stats = [dict((k, int(v)) for k,v in
                              (line.split() for line in r['child_cgroups'][group]['memory.stat'].splitlines())) for r in rows]
                windows[name] = {key:statistics.median(s[key]/2**20 for s in stats)
                                 for key in ('anon','file','kernel')}
                windows[name]['sample_count'] = len(rows)
            groups[group] = dict(quiet_windows_mib=windows,
                sampled_peak_current_bytes=max(int(r['child_cgroups'][group]['memory.current']) for r in resources),
                peak_bytes=max(int(r['child_cgroups'][group]['memory.peak']) for r in resources),
                final_events=resources[-1]['child_cgroups'][group]['memory.events'])
        results[plan] = dict(passed=summary['passed'], gates=summary['gates'],
            logs=summary['recovered_logs'], batches=summary['acknowledged_batches'],
            server_rss_mib=summary['server_peak_rss_mib'], node_rss_mib=summary['node_peak_rss_mib'],
            aggregate_node_rss_mib=summary['aggregate_nodes_peak_rss_mib'],
            server_cpu_seconds=summary['server_cpu_seconds'], nodes_cpu_seconds=summary['nodes_cpu_seconds'],
            server_mean_cores=summary['server_mean_cpu_equivalents'],
            segments=summary['segments_final'], journal_final_bytes=summary['journal_final_bytes'],
            residency=summary['residency'], cgroups=groups,
            latency_ms=summary['latency_ms']['all'],
            query_p99_ms={k:v['all']['latency_ms']['p99'] for k,v in observation['queries'].items()},
            visibility=observation['visibility'], rates=observation['rates'],
            producer_lateness_ms=observation['producer_lateness_ms'],
            final_progress=observation['progress_final'],
            custody_and_query=read(directory/'query-verdicts.json'))
        for filename in ('summary.json','resources.json','query-verdicts.json','cleanup.json','environment.json'):
            path = directory/filename
            with path.open('rb') as stream:
                sources[str(path.relative_to(ROOT))] = hashlib.file_digest(stream,'sha256').hexdigest()
    fault_dir = BASE/'memory/service-recovery/faults'
    results['recovery'] = read(fault_dir/'summary.json')
    results['limits'] = ['Two ordered descriptive native cases, no matched causal speedup.',
                         'Quiet interval is 180 seconds; not the registered soak.',
                         'Injected ENOSPC/EIO and process kills; not physical power loss.',
                         'Full-chain oracles use recovered raw records; source and ACK hash checks are separate.',
                         'No production settings changed or deployment qualified.']
    results['source_sha256'] = sources
    (out/'summary.json').write_text(json.dumps(results, indent=2)+'\n')
    print(json.dumps({name:{k:v for k,v in item.items() if k in ('passed','logs','batches','server_rss_mib','segments','residency','query_p99_ms')}
                      for name,item in results.items() if name in ('scan','walk','recovery')},indent=2))


if __name__ == '__main__':
    main()
