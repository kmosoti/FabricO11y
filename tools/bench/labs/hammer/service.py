#!/usr/bin/env python3
"""One contained current-source pressure cell; semantic gates and service diagnostics."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT/'tools'))
sys.path.insert(0, str(ROOT/'tools/bench'))
import resource_group
import observe_dev_small as observation
# The observer implementation and independent oracle are reused unchanged;
# its native helpers resolve to this pressure harness, not the historical pilot.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import native
observation.native = native

BUILD = ROOT/'docs/experiments/benchmarks/data/native-frontier-01/memory/service-recovery/build/build.json'


class Observer(observation.Observer):
    def __init__(self, deadline):
        super().__init__()
        self.deadline = deadline

    def stop(self):
        self.done.set()
        for thread in self.threads:
            thread.join(max(.01, min(12, self.deadline-time.monotonic()-20)))
        if any(t.is_alive() for t in self.threads):
            raise TimeoutError('consumer thread did not stop within absolute deadline')

    def finish(self, *args):
        if time.monotonic() >= self.deadline-100:
            raise TimeoutError('independent verification reserve unavailable')
        super().finish(*args)
        if time.monotonic() >= self.deadline-20:
            raise TimeoutError('independent verification exceeded cleanup reserve')
        root, summary = args[:2]
        # These historical qualification objectives remain evidence, while
        # exact answers, custody, replay and controls stay semantic gates.
        summary.setdefault('service_objectives', {}).update({name:summary['gates'].pop(name)
            for name in ('producer_lag_p99_le_100ms', 'all_visibility_targets_returned',
                         'source_to_visible_le_30s')})
        summary['passed'] = all(summary['gates'].values())
        native.dump(root/'summary.json', summary)


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def inventory(root):
    members = {}
    for path in sorted(root.rglob('*')):
        if path.is_symlink():
            raise RuntimeError('linked evidence member')
        if path.is_file():
            members[str(path.relative_to(root))] = dict(bytes=path.stat().st_size, sha256=digest(path))
        elif path.is_dir():
            members[str(path.relative_to(root))] = dict(type='directory')
        else:
            raise RuntimeError('special evidence member')
    return members


def frozen_build(bins):
    frozen = json.loads(BUILD.read_text())
    source = {name:value for name,value in frozen['source_sha256'].items()
              if name.endswith('.rs') or Path(name).name in ('Cargo.toml','Cargo.lock')}
    # Match the retained build's Rust/manifest scope exactly, including
    # untracked files inside that scope and excluding unrelated tooling crates.
    actual_sources = ({path for directory in (ROOT/'src', ROOT/'crates', ROOT/'examples')
                       for path in directory.rglob('*.rs')} |
                      {ROOT/'Cargo.toml', ROOT/'Cargo.lock'} |
                      set((ROOT/'crates').glob('*/Cargo.toml')))
    actual_paths = {str(path.relative_to(ROOT)) for path in actual_sources}
    if actual_paths != set(source):
        raise RuntimeError('Rust/manifest source set differs from retained build')
    if any(digest(ROOT/name) != value for name,value in source.items()):
        raise RuntimeError('Rust/manifest source differs from retained build')
    hashes = {name:digest(bins/name) for name in ('fabric-server','fabric-node','examples/server_dump')}
    if any(value != frozen['binaries'][name] for name,value in hashes.items()):
        raise RuntimeError('binary differs from retained current-source build')
    return hashes, source


def residency(samples, epoch):
    def median(begin, end):
        values = [r['server']['rss_kib']/1024 for r in samples
                  if begin <= (r['wall_ns']-epoch)/1e9 < end]
        return statistics.median(values) if values else None
    first, last = median(180,240), median(300,360)
    return dict(first_quiet_median_rss_mib=first, final_quiet_median_rss_mib=last,
                h1_residency_compatible=last <= first*1.10+16 if first and last else None,
                boundary='finite quiet period with queries and metric collection active; diagnostic')


def compact(work, out):
    for path in sorted(work.iterdir()):
        if path.is_file() and path.name != 'recovered.jsonl' and path.suffix in ('.json','.gz','.out','.err'):
            target = out/path.name
            if path.suffix == '.gz':
                with gzip.open(path,'rb') as source, target.open('wb') as raw, gzip.GzipFile(fileobj=raw,mode='wb',mtime=0) as dest:
                    shutil.copyfileobj(source,dest)
                with gzip.open(path,'rb') as source, gzip.open(target,'rb') as copied:
                    if hashlib.file_digest(source,'sha256').hexdigest() != hashlib.file_digest(copied,'sha256').hexdigest():
                        raise RuntimeError('compact compressed evidence exact readback mismatch')
            else:
                shutil.copyfile(path,target)
                if digest(path) != digest(target):
                    raise RuntimeError('compact evidence exact readback mismatch')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--plan', choices=['scan','walk'])
    parser.add_argument('--out', type=Path)
    parser.add_argument('--bin-dir', type=Path)
    parser.add_argument('--id')
    parser.add_argument('--seed', type=int, default=2703163393)
    parser.add_argument('--controls', action='store_true')
    args = parser.parse_args()
    resource_group.require_limits()
    if args.controls:
        expected = {'a':('same',0,'normal')}
        assert not any(native.compare(expected, {'a':'same'}).values())
        assert native.compare(expected, {})['missing'] == 1
        assert native.compare(expected, {'a':'changed'})['changed'] == 1
        print(json.dumps(dict(observation.controls(), changed_source_rejected=True)))
        return 0
    if not all([args.plan,args.out,args.bin_dir,args.id]):
        parser.error('plan, out, bin-dir and id required')
    if not args.id.replace('-', '').replace('_', '').isalnum():
        parser.error('invalid fresh identifier')
    started = time.monotonic()
    deadline = started+1400
    temporary = Path(os.environ['TMPDIR']).resolve(strict=True)
    temporary.relative_to((resource_group.STORAGE/'scratch').resolve(strict=True))
    coordinated = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True)
    coordinated.relative_to(temporary)
    work = coordinated/('service-'+args.id)
    if work.exists() or work.is_symlink():
        raise RuntimeError('fixed scratch id already exists')
    if shutil.disk_usage(coordinated).free < 16*2**30:
        raise RuntimeError('16GiB free disk admission required')
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    bins = args.bin_dir.resolve(strict=True)
    hashes, sources = frozen_build(bins)
    cpus = sorted(os.sched_getaffinity(0))
    if len(cpus)<6:
        raise RuntimeError('six affinity CPUs required')
    parent = native.cgroups.delegate()
    page = os.sysconf('SC_PAGE_SIZE')
    server_max, server_high = (n//page*page for n in (4_000_000_000,3_000_000_000))
    server, sl = native.cgroups.subgroup(parent,'hammer-server',server_max,server_high,256,4)
    nodes, nl = native.cgroups.subgroup(parent,'hammer-nodes',2**30,768*2**20,512,2)
    freeze = [Path(__file__), Path(native.__file__), Path(observation.__file__),
              Path(native.cgroups.__file__), Path(resource_group.__file__),
              Path(native.query_oracle.__file__), ROOT/'tools/qualification/delivery_faults.py']
    native.dump(out/'environment.json',dict(command=sys.argv,
        revision=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
        binary_sha256=hashes, source_sha256=sources, retained_build_sha256=digest(BUILD),
        server_cpus=cpus[:4],node_cpus=cpus[4:6],parent=str(parent),
        child_limits=dict(server=sl,nodes=nl),scratch=str(work),deadline_seconds=1400,
        verification_reserve_seconds=800,live_scratch_ceiling_bytes=8*2**30,
        server_disk_budget_bytes=100_000_000_000,replay_ceiling_bytes=3*2**30,
        source_offer_logs=1260000,source_body_bytes=900,phase_logs_per_second=[1000,4000,16000],
        phase_seconds=60,quiet_seconds=180,seed=args.seed,
        uname=list(os.uname()),free_bytes=shutil.disk_usage(temporary).free,
        harness_sha256={str(p.relative_to(ROOT)):digest(p) for p in freeze}))
    shutil.copyfile(BUILD,out/'retained-build.json')
    (out/'working-tree.diff').write_bytes(subprocess.check_output(['git','diff','HEAD'],cwd=ROOT))
    for path in freeze:
        name = str(path.relative_to(ROOT)).replace('/','__')+'.gz'
        (out/name).write_bytes(gzip.compress(path.read_bytes(),mtime=0))
    native.dump(out/'controls.json',observation.controls())
    observer = Observer(deadline)
    status = 'failed'
    try:
        os.sched_setaffinity(0,cpus[4:6])
        result = native.trial(work,bins,'pressure',cpus[:4],cpus[4:6],observer,args.plan,
                              dict(server=server,nodes=nodes),deadline,args.seed)
        if frozen_build(bins) != (hashes,sources):
            raise RuntimeError('retained sources or binaries changed during cell')
        events = {name:dict(line.split() for line in (group/'memory.events').read_text().splitlines())
                  for name,group in [('server',server),('nodes',nodes)]}
        result['gates']['no_child_cgroup_oom'] = all(int(e['oom'])==0 and int(e['oom_kill'])==0 for e in events.values())
        result['gates']['server_memory_limit_4gb'] = (server/'memory.max').read_text().strip()==str(server_max) and int((server/'memory.peak').read_text())<=server_max
        result['gates']['no_child_swap'] = all((g/'memory.swap.current').read_text().strip()=='0' for g in (server,nodes))
        samples = json.loads((work/'resources.json').read_text())
        result['gates']['multiple_publications'] = max(r['segments'] for r in samples)>=2
        result['gates']['drained_final_progress'] = all(p['pending_batches']==0 and p['file_backlog_bytes']==0 for p in samples[-1]['progress'])
        result['residency'] = residency(samples,observer.epoch)
        result['passed'] = all(result['gates'].values())
        native.dump(work/'summary.json',result)
        status = 'passed' if result['passed'] else 'failed'
    except BaseException as exc:
        native.dump(out/'failure.json',dict(error=repr(exc)))
    finally:
        preservation = dict(removed=False,original=str(work))
        try:
            if work.exists():
                observer.archive(work)
                if status=='passed':
                    compact(work,out)
                    # Full compact evidence is checked before deleting success state.
                    if native.footprint(out)>2*2**30-2**20:
                        raise RuntimeError('compact evidence exceeds coordinator 2GiB allowance')
                    shutil.rmtree(work)
                    preservation = dict(removed=True,success_raw_evidence_bytes=native.footprint(out))
                else:
                    native.dump(out/'failure-tree-members.json',inventory(work))
                    preservation['inventory'] = str(out/'failure-tree-members.json')
        except BaseException as exc:
            status = 'failed'
            preservation.update(error=repr(exc),removed=False,original=str(work))
            if work.exists():
                native.dump(out/'failure-tree-members.json',inventory(work))
        native.dump(out/'cgroup-final.json',native.cgroups.snapshot(parent))
        for group in (server,nodes):
            if not (group/'cgroup.procs').read_text().strip():
                group.rmdir()
        native.dump(out/'cleanup.json',dict(status=status,elapsed_s=time.monotonic()-started,
            logical_work_bytes=native.footprint(work) if work.exists() else 0,**preservation))
        # This manifest follows the final resource and cleanup writes.
        native.dump(out/'compact-sha256.json',{p.name:digest(p) for p in out.iterdir()
                    if p.is_file() and p.name!='compact-sha256.json'})
    return 0 if status=='passed' else 1


if __name__=='__main__':
    raise SystemExit(main())
