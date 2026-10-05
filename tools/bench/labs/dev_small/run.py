#!/usr/bin/env python3
"""Coordinator-dispatched native cells for the registered second lab wave."""
import argparse
import gzip
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT/'tools'))
from resource_group import require_limits, STORAGE
import native
import measurement
import archive

DATA = ROOT/'docs/experiments/benchmarks/data/dev-small-labs-run-02'
PROTOCOL = ROOT/'docs/experiments/benchmarks/dev-small-labs-screen-protocol.md'
CELLS = {
    'm0':('query',1,(10,30,10),'H0','scan',False),
    'c1-1':('memory',20,(1000,3000,1000),'H0','off',False),
    'c1-2':('memory',20,(1000,3000,1000),'H256','scan',False),
    'c1-3':('memory',20,(1000,3000,1000),'H0','scan',False),
    'c1-4':('memory',20,(1000,3000,1000),'H256','off',False),
    'c1-5':('memory',20,(100,300,100),'H256','scan',False),
    'c1-6':('memory',1,(10,30,10),'H256','scan',False),
    'q1-fresh':('query',1,(10,30,10),'H0','q1-fresh',False),
    'q1-seeded':('query',1,(10,30,10),'H0','q1-seeded',True),
}


def binaries(bins):
    return {name:archive.sha(bins/name) for name in
            ('fabric-server','fabric-node','examples/server_dump','examples/lab_history_seed')}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--cell',choices=CELLS)
    ap.add_argument('--controls',action='store_true')
    args = ap.parse_args()
    require_limits()
    tmp = Path(os.environ['TMPDIR']).resolve()
    if not tmp.is_relative_to(STORAGE/'scratch'):
        raise RuntimeError('launcher data-drive scratch required')
    if args.controls:
        checks = {'native':native.controls(), 'measurement':measurement.controls(),
                  'archive':archive.controls(tmp)}
        print(json.dumps(checks),flush=True)
        return
    if not args.cell:
        ap.error('--cell or --controls required')
    lab,nodes,rates,history,mode,near = CELLS[args.cell]
    labdir = DATA/lab
    labdir.mkdir(parents=True,exist_ok=True)
    corpus = labdir/'corpus'
    corpus.mkdir(exist_ok=True)
    destination = labdir/args.cell
    destination.mkdir(exist_ok=False)
    if native.footprint(labdir) > 40*2**20:
        raise RuntimeError('less than 10MiB evidence headroom; stop before workload')
    bins = Path(os.environ['CARGO_TARGET_DIR'])/'release'
    hashes = binaries(bins)
    frozen = DATA/'binaries.json'
    if frozen.exists() and json.loads(frozen.read_text()) != hashes:
        raise RuntimeError('campaign binaries changed')
    if not frozen.exists():
        native.dump(frozen,hashes)
    cpus = sorted(os.sched_getaffinity(0))
    if len(cpus)<4:
        raise RuntimeError('four CPUs required')
    work = tmp/args.cell
    environment = {'command':sys.argv,'cell':args.cell,'nodes':nodes,'rates':rates,
        'history':history,'near_rotation':near,'timed_queries':mode,'binaries':hashes,
        'revision':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
        'protocol_sha256':archive.sha(PROTOCOL),'server_cpus':cpus[:2],'client_cpus':cpus[2:4],
        'storage':str(work),'uname':list(os.uname()),
        'sources':{str(p.relative_to(ROOT)):archive.sha(p) for p in
                   list(Path(__file__).parent.glob('*.py'))+[ROOT/'tools/bench/observe_dev_small.py',ROOT/'tools/qualification/query_oracle.py']}}
    native.dump(destination/'environment.json',environment)
    shutil.copyfile(PROTOCOL,destination/'protocol.txt')
    observer = measurement.Observer(mode,visibility_jitter=args.cell.startswith('q1'),native_module=native)
    status = 'interrupted'
    began = time.monotonic()
    try:
        os.sched_setaffinity(0,cpus[2:4])
        summary = native.trial(work,bins,'development' if nodes==1 else 'small',cpus[:2],cpus[2:4],
            observer,query_plan='scan',nodes_count=nodes,rates=rates,history=history,near_rotation=near)
        native.dump(work/'environment.json',environment)
        summary['gates']['no_unexpected_retries'] = summary['non_ack_attempts']==0
        summary['gates']['measurement_valid'] = summary.get('measurement',{}).get('valid_for_timing',False)
        summary['passed'] = all(summary['gates'].values())
        native.dump(work/'summary.json',summary)
        native.dump(work/'artifact-manifest.json',{'files':{
            p.name:archive.sha(p) for p in work.iterdir()
            if p.is_file() and p.suffix in ('.json','.gz','.jsonl')
            and p.name not in ('artifact-manifest.json','recovered.jsonl')}})
        if binaries(bins)!=hashes:
            raise RuntimeError('binary changed during cell')
        audit = subprocess.run([sys.executable,str(Path(__file__).with_name('audit.py')),'--cell',str(work)],check=False)
        status = 'passed' if summary['passed'] and audit.returncode==0 else 'failed'
    except BaseException as exc:
        native.dump(destination/'failure.json',{'error':repr(exc)})
        raise
    finally:
        if work.exists():
            observer.archive(work)
            if (work/'resources.json').exists():
                with (work/'resources.json').open('rb') as src,gzip.open(work/'resources.json.gz','wb',compresslevel=9) as dst:
                    shutil.copyfileobj(src,dst)
            if (work/'sources.jsonl.gz').exists():
                archive.preserve_sources(work/'sources.jsonl.gz',destination,corpus,tmp)
            if (work/'seed-ledger.jsonl').exists():
                seed = archive.store_gzip(work/'seed-ledger.jsonl',corpus)
                native.dump(destination/'seed-layout.json',{'ledger':'../corpus/'+seed.name,
                    'raw_sha256':archive.sha(work/'seed-ledger.jsonl')})
            excluded={'recovered.jsonl','resources.json','sources.jsonl.gz'}
            if (work/'data-clocks.jsonl.gz').exists():
                try:
                    archive.preserve_clocks(work/'data-clocks.jsonl.gz',destination,tmp)
                    excluded.add('data-clocks.jsonl.gz')
                except ValueError as exc:
                    status='failed'
                    native.dump(destination/'clock-preservation-failure.json',{'error':str(exc)})
                    for name in ('clock-layout.json','data-clock-fields.jsonl.gz'):
                        (destination/name).unlink(missing_ok=True)
            for p in work.iterdir():
                if p.is_file() and p.name not in excluded and p.suffix in ('.json','.gz','.err','.out'):
                    name = 'prearchive-manifest.json' if p.name=='artifact-manifest.json' else p.name
                    shutil.copyfile(p,destination/name)
            retained = native.footprint(labdir)
            if retained>50*2**20:
                status='failed'
                native.dump(destination/'preservation-failure.json',{'bytes':retained,'limit':50*2**20})
            native.dump(destination/'artifact-manifest.json',{p.name:archive.sha(p) for p in destination.iterdir() if p.is_file() and p.name!='artifact-manifest.json'})
            size=native.footprint(work)
            if status=='passed':
                shutil.rmtree(work)
            native.dump(destination/'cleanup.json',{'status':status,'work':str(work),
                'removed':not work.exists(),'logical_bytes':size,'elapsed_s':time.monotonic()-began,
                'retained_lab_bytes':native.footprint(labdir),'failure_moves_with_launcher':status!='passed'})
    print(json.dumps({'cell':args.cell,'status':status}),flush=True)
    raise SystemExit(0 if status=='passed' else 1)


if __name__=='__main__':
    main()
