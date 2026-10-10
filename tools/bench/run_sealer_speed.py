#!/usr/bin/env python3
"""Registered, sequential speed ablation with frozen binaries and custody checks."""
import argparse
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import sys
import tarfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from resource_group import STORAGE, require_limits
import run_ingestion_memory as memory

ROOT = Path(__file__).resolve().parents[2]
DATA = ROOT / 'docs/experiments/benchmarks/data/sealer-speed-run-01'
BIN = STORAGE / 'scratch/sealer-speed-binaries'


def snapshot(dest):
    dest.mkdir(parents=True)
    (dest/'working-tree.diff').write_bytes(subprocess.check_output(['git','diff','HEAD'],cwd=ROOT))
    with tarfile.open(dest/'source-snapshot.tar.gz', 'x:gz') as archive:
        names = subprocess.check_output(['git','ls-files','-z','--cached','--others','--exclude-standard'],cwd=ROOT)
        for name in names.decode().split('\0'):
            p = ROOT / name
            if name and p.is_file() and (p.suffix=='.rs' or p.name in ('Cargo.toml','Cargo.lock')):
                # Frozen evidence copies are not build inputs.
                if not name.startswith('docs/experiments/benchmarks/data/'):
                    archive.add(p,arcname=name)


def phases_summary(path):
    rows = [json.loads(s) for s in path.read_text().splitlines()]
    roots = [r for r in rows if r['phase']=='bounded_segment_build']
    ledger_rows = len(rows)
    rows = [r for r in rows if any(r['thread']==b['thread'] and
            b['start_ns'] <= r['start_ns'] and
            r['start_ns']+r['wall_ns'] <= b['start_ns']+b['wall_ns'] for b in roots)]
    # A stack over start order subtracts only immediate children, per thread.
    stacks = {}
    for row in sorted(rows,key=lambda r:r['start_ns']):
        stack = stacks.setdefault(row['thread'],[])
        while stack and stack[-1]['depth'] >= row['depth']:
            stack.pop()
        row['exclusive_ns'] = row['wall_ns']
        row['exclusive_cpu_ns'] = row['thread_cpu_ns']
        if stack:
            stack[-1]['exclusive_ns'] -= row['wall_ns']
            stack[-1]['exclusive_cpu_ns'] -= row['thread_cpu_ns']
        stack.append(row)
    totals = {}
    for row in rows:
        if row['phase'] == 'bounded_segment_build' or row['depth'] > 0:
            t = totals.setdefault(row['phase'],dict(count=0,inclusive_ns=0,exclusive_ns=0,exclusive_cpu_ns=0))
            t['count'] += 1
            t['inclusive_ns'] += row['wall_ns']
            t['exclusive_ns'] += max(0,row['exclusive_ns'])
            t['exclusive_cpu_ns'] += max(0,row['exclusive_cpu_ns'])
    return {'ledger_rows':ledger_rows,'build_rows':len(rows),'phases':totals}


def profile(variant=None):
    assert variant in (None, 'candidate')
    dest = DATA/('profile' if variant is None else 'profile-candidate')
    snapshot(dest)
    binary = Path(os.environ['CARGO_TARGET_DIR'])/'release/examples/responsibility_probe'
    cases = [(65536,False),(65536,True),(262144,True)] if variant is None else [(262144,True)]
    for count, enabled in cases:
        name = f'{count}-phases-{int(enabled)}'
        work = Path(os.environ['FABRIC_SCRATCH_ROOT'])/('sealer-profile-'+name)
        evidence = dest/name
        evidence.mkdir(); work.mkdir()
        env = dict(os.environ,BENCH_RECORDS=str(count),BENCH_BODY_SIZE='1024',BENCH_ORDER='shuffled',
                   BENCH_BUILDER='bounded',BENCH_PHASES=str(int(enabled)),BENCH_OBSERVER='detailed')
        command = [str(binary),str(work),'memory','1024']
        memory.dump(evidence/'command.json',{'command':command,'env':{k:v for k,v in env.items() if k.startswith('BENCH_')},
                    'binary_sha256':memory.digest(binary)})
        print('profile',name,flush=True)
        status = None
        try:
            with (evidence/'stdout.jsonl').open('w') as out, (evidence/'stderr.txt').open('w') as err:
                status = subprocess.run(command,env=env,stdout=out,stderr=err,timeout=300).returncode
            assert status == 0, status
            shutil.copy(work/'phases.jsonl', evidence/'phases.jsonl')
            summary = phases_summary(evidence/'phases.jsonl')
            memory.dump(evidence/'phases-summary.json',summary)
            print(json.dumps(summary),flush=True)
        finally:
            if status == 0:
                size = sum(p.stat().st_size for p in work.rglob('*') if p.is_file())
                shutil.rmtree(work)
                memory.dump(evidence/'cleanup.json',{'logical_bytes':size,'removed':not work.exists()})
            else:
                memory.dump(evidence/'failure.json',{'exit':status,'retained':str(work)})


def report():
    cells = []
    for action in ('compare','binary'):
        results = json.loads((DATA/(action+'-complete.json')).read_text())['results']
        for count in sorted({r['count'] for r in results}):
            for variant in sorted({r['speed_variant'] for r in results}):
                rows = [r for r in results if r['count']==count and r['speed_variant']==variant]
                values = {
                    'wall_ms':[r['measurement']['wall_ns']/1e6 for r in rows],
                    'cpu_ms':[r['measurement']['cpu_ns']/1e6 for r in rows],
                    'heap_mib':[r['incremental_heap_bytes']/2**20 for r in rows],
                    'rss_mib':[r['whole_process_rss_peak_kib']/1024 for r in rows],
                    'read_mib':[r['measurement']['proc_io_delta']['rchar']/2**20 for r in rows],
                    'written_mib':[r['measurement']['proc_io_delta']['wchar']/2**20 for r in rows],
                }
                cells.append(dict(action=action,count=count,variant=variant,n=len(rows),
                    max_heap_bytes=max(r['incremental_heap_bytes'] for r in rows),
                    **{key:statistics.median(value) for key,value in values.items()}))
    queries = json.loads((DATA/'pending-complete.json').read_text())['results']
    verdicts = [v for r in queries for v in r['query_verdicts']]
    cleanups = [json.loads(p.read_text()) for p in DATA.rglob('cleanup.json')]
    value = {'cells':cells,'query_samples':len(verdicts),
             'queries_all_passed':all(v['verdict']['passed'] for v in verdicts),
             'query_negative_controls':sum(len(r['negative_controls_rejected']) for r in queries),
             'during_samples':[r['exactness']['during_samples'] for r in queries],
             'cleanup_receipts':len(cleanups),'all_scratch_removed':all(r['removed'] for r in cleanups),
             'cleaned_logical_bytes':sum(r['logical_bytes'] for r in cleanups)}
    def gates(baseline, candidate):
        return {'speed':candidate['wall_ms'] <= baseline['wall_ms']*0.9,
                'heap_growth':candidate['max_heap_bytes'] <= baseline['max_heap_bytes']*1.1}
    control = {'wall_ms':100,'max_heap_bytes':100}
    assert not gates(control, control)['speed']
    assert not gates(control, {'wall_ms':80,'max_heap_bytes':120})['heap_growth']
    value['gate_negative_controls_rejected'] = ['no_speed_gain','heap_regression']
    value['gates'] = {}
    for count in (65536,262144):
        b = next(c for c in cells if c['action']=='compare' and c['count']==count and c['variant']=='baseline')
        c = next(c for c in cells if c['action']=='compare' and c['count']==count and c['variant']=='candidate')
        result = gates(b,c)
        if count==262144:
            result['retain_97_percent_reduction'] = c['max_heap_bytes'] <= 1_242_865_595*0.03
        value['gates'][str(count)] = result
    memory.dump(DATA/'summary.json', value)
    print(json.dumps(value,indent=2))
    assert all(all(g.values()) for g in value['gates'].values()), 'registered finite gate failed'


def main():
    require_limits()
    parser = argparse.ArgumentParser()
    parser.add_argument('action',choices=['profile','freeze','compare','binary','pending','report','cleanup'])
    parser.add_argument('--variant')
    args = parser.parse_args()
    if args.action=='profile':
        profile(args.variant)
    elif args.action=='report':
        report()
    elif args.action=='freeze':
        assert args.variant in ('baseline','binary-only','candidate')
        snapshot(DATA/args.variant)
        BIN.mkdir(parents=True,exist_ok=True)
        binary = BIN/args.variant
        assert not binary.exists()
        shutil.copy2(Path(os.environ['CARGO_TARGET_DIR'])/'release/examples/responsibility_probe',binary)
        memory.dump(DATA/args.variant/'binary.json',{'path':str(binary),'sha256':memory.digest(binary)})
    elif args.action in ('compare','binary','pending'):
        results = []
        mode = 'pending' if args.action=='pending' else 'memory'
        candidates = ('baseline','binary-only' if args.action=='binary' else 'candidate')
        counts = [262144] if args.action=='binary' else ([65536,262144] if mode=='memory' else [65536])
        for count in counts:
            for repeat in range(1,4):
                pair = {}
                for variant in (candidates if repeat%2 else tuple(reversed(candidates))):
                    dest = DATA/variant/args.action
                    dest.mkdir(exist_ok=True)
                    # Each call includes exact source-row/raw-custody checks and owned cleanup.
                    result = memory.run(dest,BIN/variant,mode,count,'bounded',repeat)
                    result['speed_variant'] = variant
                    pair[variant] = result
                    results.append(result)
                if mode=='memory':
                    assert pair['baseline']['exactness']['files'] == pair[candidates[1]]['exactness']['files']
        memory.dump(DATA/(args.action+'-complete.json'),{'exit':0,'results':results})
    else:
        for variant in ('baseline','binary-only','candidate'):
            (BIN/variant).unlink()
        BIN.rmdir()
        memory.dump(DATA/'binary-cleanup.json',{'removed':not BIN.exists()})


if __name__=='__main__':
    main()
