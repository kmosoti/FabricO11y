#!/usr/bin/env python3
"""Coordinator-dispatched native cells for the registered second lab wave."""
import argparse
import gzip
import base64
import hashlib
from contextlib import ExitStack
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import copy
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT/'tools'))
from resource_group import require_limits, STORAGE
sys.path.insert(0, str(Path(__file__).parent.parent/'query_compare'))
import consumer
sys.path.insert(0, str(Path(__file__).parent.parent/'dev_small'))
import coupled_c5_native as native
import measurement
import archive
import coupled_c5_archive as c5_archive

DATA = ROOT/'docs/experiments/benchmarks/data/catalog-borrowed-c5-run-01'
PROTOCOL = ROOT/'docs/experiments/benchmarks/dev-small-labs-screen-protocol.md'
ADDENDUM = ROOT/'docs/experiments/benchmarks/dev-small-labs-screen-r2.md'
CAMPAIGN = ROOT/'docs/experiments/benchmarks/coupled-c5-protocol.md'
CELLS = {f'w{workers}-r{run}-{queries}': ('memory',20,(1000,3000,1000),'H256','walk',False)
         for workers in (1,2) for run in (8,16,32) for queries in ('off','on')}

EVIDENCE_CAP = 20*2**20
CELL_ARCHIVE_PROJECTION = 4*2**20
AGGREGATE_CAP = 1_894_879_232
CAPACITY_CAP = 872_415_232
CAPACITY_PRIOR = 847_081_472

def usage(root):
    files = {}
    for p in root.rglob('*'):
        try:
            if p.is_file():
                st=p.stat();files[st.st_dev,st.st_ino]=(st.st_size,st.st_blocks*512)
        except FileNotFoundError:pass
    return max(sum(v[0] for v in files.values()),sum(v[1] for v in files.values()))

def project_inputs(tmp):
    work=tmp/'c5-input-projection'
    work.mkdir(exist_ok=False)
    started=time.monotonic();counts=[0]*20;hashers=[hashlib.sha256() for _ in range(20)];samples=0
    try:
        with ExitStack() as stack:
            outputs=[]
            for node in range(20):
                raw=stack.enter_context((work/f'node{node:02}.log.gz').open('wb'))
                outputs.append(stack.enter_context(gzip.GzipFile(filename='',mode='wb',fileobj=raw,compresslevel=9,mtime=0)))
            accumulators=[0]*20
            for tick in range(1800):
                rate=(1000,3000,1000)[tick//600]
                for node in range(20):
                    accumulators[node]+=rate
                    count,accumulators[node]=divmod(accumulators[node],200)
                    for index in range(count):
                        tag=f'{node:02}:{tick:04}:{index:02}'
                        padding=('R'*900 if (tick+index)%2==0 else base64.b85encode(hashlib.shake_256(f'2703163393:{tag}'.encode()).digest(720)).decode())
                        body=('load-'+tag+' '+padding)[:900]
                        encoded=body.encode()
                        if len(encoded)!=900:raise RuntimeError('projection body length')
                        if counts[node]%1000==0:
                            if hashlib.sha256(encoded).hexdigest()!=c5_archive.body_hash(tag):raise RuntimeError('projection body helper mismatch')
                            samples+=1
                        line=encoded+b'\n';outputs[node].write(line);hashers[node].update(line);counts[node]+=1
                if tick%100==0 and (time.monotonic()-started>80 or usage(work)>1024**3 or shutil.disk_usage(STORAGE).free<16*2**30):
                    raise RuntimeError('projection time/scratch/free-space bound')
        files=[]
        for node in range(20):
            path=work/f'node{node:02}.log.gz'
            with gzip.open(path,'rb') as stream:
                if hashlib.file_digest(stream,'sha256').hexdigest()!=hashers[node].hexdigest():raise RuntimeError('projection decoded input readback')
            st=path.stat();files.append({'node':node,'lines':counts[node],'decoded_bytes':counts[node]*901,
                'decoded_sha256':hashers[node].hexdigest(),'gzip_sha256':archive.sha(path),'file_bytes':st.st_size,'allocated_bytes':st.st_blocks*512})
        if counts!=[15000]*20:raise RuntimeError('projection offered count drift')
        result={'scope':'exact per-node offered source-input gzip preservation; excludes seed/journal/Spool and receipts',
                'counts':counts,'total_lines':sum(counts),'body_bytes':900,'schedule_seconds':[60,60,60],'rates':[1000,3000,1000],
                'sample_body_hash_checks':samples,'files':files,'unique_file_bytes':sum(f['file_bytes'] for f in files),
                'unique_allocated_bytes':sum(f['allocated_bytes'] for f in files),'input_sha_readbacks':True,
                'source_sha256':{str(p.relative_to(ROOT)):archive.sha(p) for p in [Path(__file__),Path(native.__file__),Path(c5_archive.__file__)]}}
    finally:
        shutil.rmtree(work)
    result['scratch_removed']=not work.exists();result['elapsed_s']=time.monotonic()-started
    DATA.mkdir(parents=True,exist_ok=True);native.dump(DATA/'projection.json',result)
    return result

def failure_projection():
    path=DATA/'projection.json'
    if not path.exists():raise RuntimeError('C5 admission requires measured input-storage projection')
    result=json.loads(path.read_text())
    expected={str(p.relative_to(ROOT)):archive.sha(p) for p in [Path(__file__),Path(native.__file__),Path(c5_archive.__file__)]}
    if result.get('source_sha256')!=expected or result.get('total_lines')!=300000 or result.get('body_bytes')!=900 or not result.get('input_sha_readbacks') or not result.get('scratch_removed'):
        raise RuntimeError('projection fixture/source/readback mismatch')
    return max(result['unique_file_bytes'],result['unique_allocated_bytes'])

def admission(reserve):
    import campaign_summary as inventory
    report=inventory.evidence_inventory(ROOT/'docs/experiments/benchmarks/data')
    owned=usage(DATA)
    aggregate=report['catalog_complete_owned_persistent']
    total=max(aggregate['unique_inode_bytes'],report['aggregate_allocated_bytes_including_object_links'])
    capacity=max(CAPACITY_PRIOR+owned,report['capacity_mandatory_lower_bound']['allocated_block_bytes'],report['capacity_mandatory_lower_bound']['unique_inode_bytes'])
    result={'owned_bytes':owned,'reserve_bytes':reserve,'archive_projection_bytes':CELL_ARCHIVE_PROJECTION,
            'failure_input_preservation_bytes':failure_projection(),'owned_cap_bytes':EVIDENCE_CAP,
            'aggregate_bytes':total,'aggregate_cap_bytes':AGGREGATE_CAP,'capacity_bytes':capacity,'capacity_cap_bytes':CAPACITY_CAP}
    result['storage_subset_fits']=owned+reserve<=EVIDENCE_CAP and total+reserve<=AGGREGATE_CAP and capacity+reserve<=CAPACITY_CAP
    result['full_failure_bound_established']=False
    result['admitted']=False
    result['reason']='measured input preservation exceeds allocation' if not result['storage_subset_fits'] else 'input subset fits but full failed-state bound remains unestablished'
    return result

def enforce_live():
    record=admission(failure_projection())
    if not record['admitted']:raise RuntimeError('C5 live evidence/failure projection exceeds prospective allocation')

def cadence_ok(rows, epoch):
    timed = [r for r in rows if r['shape'] != 'visibility']
    return len(timed) == 200 and all(
        r['scheduled_ns'] == epoch+i*10**9 and
        r['shape'] == ('recent_logs','absent_text','cpu_metrics')[i%3]
        for i,r in enumerate(timed))


class Observer(consumer.Observer):
    def start(self, api, epoch, nodes):
        if self.cell == 'off':
            self.api, self.epoch = api, epoch
        else:
            super().start(api, epoch, nodes)

    def seed_info(self, root, summary):
        super().seed_info(root, summary)
        self.build_samples = []
        import threading
        def monitor():
            previous = None
            while not self.done.is_set():
                paths = sorted(p.name for p in (root/'state/segments').glob('.building-*'))
                if paths != previous:
                    self.build_samples.append({'mono_ns':time.monotonic_ns(),'building':paths})
                    previous = paths
                self.done.wait(.005)
        thread = threading.Thread(target=monitor)
        thread.start()
        self.threads.append(thread)

    def finish(self, root, summary, samples, events):
        super().finish(root, summary, samples, events)
        summary['gates']['fixed_cadence_exact'] = self.cell == 'off' or cadence_ok(self.queries,self.epoch)
        summary['building_samples'] = self.build_samples
        summary['peak_observed_concurrent_builds'] = max((len(r['building']) for r in self.build_samples),default=0)
        native.dump(root/'summary.json',summary)



def binaries(bins):
    return {name:archive.sha(bins/name) for name in
            ('examples/coupled_c5_server','fabric-node','examples/server_dump','examples/coupled_pending_seed')}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--cell',choices=CELLS)
    ap.add_argument('--controls',action='store_true')
    ap.add_argument('--projection',action='store_true')
    ap.add_argument('--admission-only',action='store_true')
    ap.add_argument('--bin-dir', type=Path)
    args = ap.parse_args()
    require_limits()
    tmp = Path(os.environ['TMPDIR']).resolve()
    if not tmp.is_relative_to(STORAGE/'scratch'):
        raise RuntimeError('launcher data-drive scratch required')
    if args.projection:
        print(json.dumps(project_inputs(tmp)),flush=True)
        return
    if args.admission_only:
        result=admission(CELL_ARCHIVE_PROJECTION+failure_projection())
        result.update(command_completed=True,native_child_started=False,scope='resource admission assessment, not scientific performance acceptance')
        DATA.mkdir(parents=True,exist_ok=True);native.dump(DATA/'admission-only.json',result)
        print(json.dumps(result),flush=True)
        return
    if args.controls:
        checks = {'native':native.controls(), 'measurement':measurement.controls(),
                  'archive':c5_archive.controls(tmp), 'fixed_cadence':consumer.controls()}
        rows=[{'shape':('recent_logs','absent_text','cpu_metrics')[i%3],'scheduled_ns':123+i*10**9} for i in range(200)]
        changed=copy.deepcopy(rows); changed[1]['scheduled_ns']+=1
        if not cadence_ok(rows,123) or cadence_ok(rows[:-1],123) or cadence_ok(changed,123):
            raise RuntimeError('cadence controls failed')
        checks['c5_cadence_mutants_rejected']=True
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
    projected=admission(CELL_ARCHIVE_PROJECTION+failure_projection())
    native.dump(destination/'admission.json',projected)
    if not projected['admitted']:
        raise RuntimeError('C5 not admitted: full failure preservation projection exceeds20MiB; no native child started')
    if args.bin_dir is None: ap.error('--bin-dir required for frozen variants')
    bins = args.bin_dir.resolve(strict=True)
    workers, run_mib, query_mode = args.cell.split('-')
    workers, run_mib = int(workers[1:]), int(run_mib[1:])
    description=json.loads(subprocess.check_output([str(bins/'examples/coupled_c5_server'),'--describe'],text=True))
    if description['run_mib'] != str(run_mib): raise RuntimeError('compiled run selector differs from cell')
    hashes = binaries(bins)
    frozen = DATA/f'binaries-r{run_mib}.json'
    if frozen.exists() and json.loads(frozen.read_text()) != hashes:
        raise RuntimeError('campaign binaries changed')
    if not frozen.exists():
        native.dump(frozen,hashes)
    cpus = sorted(os.sched_getaffinity(0))
    if len(cpus)<4:
        raise RuntimeError('four CPUs required')
    work = tmp/args.cell
    environment = {'command':sys.argv,'cell':args.cell,'nodes':nodes,'rates':rates,
        'history':history,'near_rotation':near,'timed_queries':query_mode,'query_plan':mode,'schedule_seconds':[60,60,60],'query_cadence_calls':200 if query_mode=='on' else 0,
        'campaign_protocol':{'path':str(CAMPAIGN.relative_to(ROOT)),'sha256':archive.sha(CAMPAIGN)},'binaries':hashes,'compiled_configuration':description,
        'revision':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
        'protocol_sha256':archive.sha(PROTOCOL),'server_cpus':cpus[:2],'client_cpus':cpus[2:4],
        'protocol_addenda':{str(ADDENDUM.relative_to(ROOT)):archive.sha(ADDENDUM)},
        'storage':str(work),'uname':list(os.uname()),
        'sources':{str(p.relative_to(ROOT)):archive.sha(p) for p in
                   list(Path(__file__).parent.glob('*.py'))+list((Path(__file__).parent.parent/'dev_small').glob('*.py'))+list((Path(__file__).parent.parent/'query_compare').glob('*.py'))+list((ROOT/'crates/fabric-server').rglob('*.rs'))+list((ROOT/'crates/fabric-frame').rglob('*.rs'))+[ROOT/'Cargo.lock',ROOT/'Cargo.toml',ROOT/'crates/fabric-server/Cargo.toml',ROOT/'tools/bench/observe_dev_small.py',ROOT/'tools/qualification/query_oracle.py',ROOT/'tools/bench/labs/completion/cgroups.py',ROOT/'tools/bench/labs/completion/enter_group.py',ROOT/'tools/resource_group.py']}}
    native.dump(destination/'environment.json',environment)
    shutil.copyfile(PROTOCOL,destination/'protocol.txt')
    shutil.copyfile(ADDENDUM,destination/'protocol-addendum.txt')
    shutil.copyfile(CAMPAIGN,destination/'campaign-protocol.txt')
    observer = Observer('off' if query_mode=='off' else mode,native_module=native)
    observer.evidence_guard=enforce_live
    sys.path.insert(0,str(ROOT/'tools/bench/labs/completion'))
    import cgroups
    parent = cgroups.delegate()
    groups = {}
    groups['server'], _ = cgroups.subgroup(parent,'server',3072*2**20,2560*2**20,512,2)
    for i in range(nodes):
        groups[f'node{i:02}'], _ = cgroups.subgroup(parent,f'node{i:02}',256*2**20,192*2**20,128,1)
    status = 'interrupted'
    began = time.monotonic()
    try:
        os.sched_setaffinity(0,cpus[2:4])
        summary = native.trial(work,bins,'development' if nodes==1 else 'small',cpus[:2],cpus[2:4],
            observer,query_plan=mode,nodes_count=nodes,rates=rates,history=history,near_rotation=near,workers=workers,groups=groups)
        native.dump(work/'environment.json',environment)
        config=(work/'server.conf').read_text()
        native.dump(work/'query-config.json', {'query_plan':dict(line.split('=',1) for line in config.splitlines())['query_plan'], 'config_sha256':archive.sha(work/'server.conf')})
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
        native.dump(work/'service-cgroups.json', cgroups.snapshot(parent))
        summary['gates']['concurrent_builds_observed'] = summary['peak_observed_concurrent_builds'] >= workers
        summary['passed'] = all(summary['gates'].values())
        native.dump(work/'summary.json',summary)
        status = 'passed' if summary['passed'] else 'failed'
    except BaseException as exc:
        native.dump(destination/'failure.json',{'error':repr(exc)})
        raise
    finally:
        if work.exists():
            observer.archive(work)
            native.dump(destination/'service-cgroups-final.json', cgroups.snapshot(parent))
            if (work/'resources.json').exists():
                with (work/'resources.json').open('rb') as src,gzip.open(work/'resources.json.gz','wb',compresslevel=9) as dst:
                    shutil.copyfileobj(src,dst)
            closed_inputs=[work/name for name in ('sources.jsonl.gz','seed-ledger.jsonl','data-clocks.jsonl.gz','recovered-hashes.jsonl.gz')]
            excluded={'recovered.jsonl','resources.json',*(p.name for p in closed_inputs)}
            if all(p.exists() for p in closed_inputs):
                c5_archive.preserve(work,destination,tmp)
            else:
                status='failed'
                native.dump(destination/'preservation-failure.json',{'missing':[p.name for p in closed_inputs if not p.exists()],
                    'scratch_retained_pending_separate_preservation_admission':True})
            for p in work.iterdir():
                if p.is_file() and p.name not in excluded and p.suffix in ('.json','.gz','.err','.out'):
                    name = 'prearchive-manifest.json' if p.name=='artifact-manifest.json' else p.name
                    shutil.copyfile(p,destination/name)
            retained = usage(labdir)
            if usage(DATA)>EVIDENCE_CAP:
                status='failed'
                native.dump(destination/'preservation-failure.json',{'bytes':retained,'limit':20*2**20})
            native.dump(destination/'artifact-manifest.json',{p.name:archive.sha(p) for p in destination.iterdir() if p.is_file() and p.name!='artifact-manifest.json'})
            size=native.footprint(work)
            if status=='passed':
                shutil.rmtree(work)
            native.dump(destination/'cleanup.json',{'status':status,'work':str(work),
                'removed':not work.exists(),'logical_bytes':size,'elapsed_s':time.monotonic()-began,
                'retained_lab_bytes':usage(labdir),'failure_moves_with_launcher':status!='passed'})
    print(json.dumps({'cell':args.cell,'status':status}),flush=True)
    raise SystemExit(0 if status=='passed' else 1)


if __name__=='__main__':
    main()
