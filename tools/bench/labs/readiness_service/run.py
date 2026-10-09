#!/usr/bin/env python3
"""One current-revision native small readiness research cell (no qualification)."""
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
import native
from evidence import preserve as bounded_archive


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


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def archive_members_match(expected, seen):
    return expected == seen


def residency(samples, epoch):
    def median(begin, end):
        values = [r['server']['rss_kib']/1024 for r in samples
                  if begin <= (r['wall_ns']-epoch)/1e9 < end]
        if len(values) < 40:
            raise RuntimeError('quiet window has insufficient observations')
        return statistics.median(values)
    first, last = median(180, 240), median(300, 360)
    return dict(first_quiet_median_rss_mib=first, final_quiet_median_rss_mib=last,
                allowed_final_mib=first*1.10+16,
                h1_residency_compatible=last <= first*1.10+16,
                boundary='finite quiet period with queries and metric collection active')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--plan', choices=['scan','walk'])
    parser.add_argument('--out', type=Path)
    parser.add_argument('--bin-dir', type=Path)
    parser.add_argument('--id')
    parser.add_argument('--controls', action='store_true')
    parser.add_argument('--preserve-mib', type=int, default=192)
    args = parser.parse_args()
    resource_group.require_limits()
    if args.controls:
        expected = {'a': ('same',0,'normal')}
        assert not any(native.compare(expected, {'a':'same'}).values())
        assert native.compare(expected, {})['missing'] == 1
        assert native.compare(expected, {'a':'changed'})['changed'] == 1
        assert archive_members_match({'a':'hash'}, {'a':'hash'})
        assert not archive_members_match({'a':'hash'}, {'a':'corrupt'})
        print(json.dumps(dict(observation.controls(), changed_source_rejected=True, corrupt_archive_readback_rejected=True)))
        return 0
    if not all([args.plan, args.out, args.bin_dir, args.id]):
        parser.error('plan, out, bin-dir and id required')
    if not args.id.replace('-', '').replace('_', '').isalnum():
        parser.error('invalid fixed fresh identifier')
    if not 1 <= args.preserve_mib <= 192:
        parser.error('preservation allowance must be 1..192 MiB')
    started = time.monotonic()
    deadline = started+750
    cap = args.preserve_mib*2**20
    temporary = Path(os.environ['TMPDIR']).resolve(strict=True)
    temporary.relative_to((resource_group.STORAGE/'scratch').resolve(strict=True))
    coordinated = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True)
    coordinated.relative_to(temporary)
    work = coordinated/'service'
    if work.exists() or work.is_symlink():
        raise RuntimeError('fixed scratch id already exists')
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    bins = args.bin_dir.resolve(strict=True)
    cpus = sorted(os.sched_getaffinity(0))
    if len(cpus)<4:
        raise RuntimeError('four affinity CPUs required')
    # delegate() moves the existing native-job monitor to supervisor. Its saved
    # outer path still covers every descendant for recursive kill/accounting.
    parent = native.cgroups.delegate()
    server, sl = native.cgroups.subgroup(parent, 'readiness-server', 2**30, 768*2**20, 256, 2)
    nodes, nl = native.cgroups.subgroup(parent, 'readiness-nodes', 512*2**20, 384*2**20, 512, 2)
    binaries = [bins/'fabric-server', bins/'fabric-node', bins/'examples/server_dump']
    hashes = {str(p.relative_to(bins)):digest(p) for p in binaries}
    frozen = json.loads((ROOT/'docs/experiments/benchmarks/data/native-frontier-01/memory/service-recovery/build/build.json').read_text())
    if any(value != frozen['binaries'][name] for name,value in hashes.items()):
        raise RuntimeError('binary differs from frozen current-source build')
    native.dump(out/'environment.json', dict(command=sys.argv, revision=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
        binary_sha256=hashes, server_cpus=cpus[:2], node_cpus=cpus[2:4], parent=str(parent),
        child_limits=dict(server=sl,nodes=nl), scratch=str(work), deadline_seconds=750,
        disk_ceiling_bytes=384*2**20, replay_ceiling_bytes=128*2**20, source_offer_logs=60000, source_body_bytes=900,
        preservation_allowance_bytes=cap, uname=list(os.uname()), free_bytes=shutil.disk_usage(temporary).free,
        harness_sha256={p.name:digest(p) for p in [Path(__file__), Path(native.__file__), Path(observation.__file__)]}))
    (out/'working-tree.diff').write_bytes(subprocess.check_output(['git','diff','HEAD'],cwd=ROOT))
    for p in [Path(__file__), Path(native.__file__), Path(observation.__file__)]:
        (out/(p.name+'.gz')).write_bytes(gzip.compress(p.read_bytes(),mtime=0))
    native.dump(out/'controls.json', observation.controls())
    observer = Observer(deadline)
    status = 'failed'
    result = None
    try:
        os.sched_setaffinity(0, cpus[2:4])
        result = native.trial(work,bins,'small',cpus[:2],cpus[2:4],observer,args.plan,
                              dict(server=server,nodes=nodes),deadline)
        if hashes != {str(p.relative_to(bins)):digest(p) for p in binaries}:
            raise RuntimeError('binary changed during cell')
        snapshots = native.cgroups.snapshot(parent)
        native.dump(out/'cgroup-final.json', snapshots)
        events = {name:dict(line.split() for line in (group/'memory.events').read_text().splitlines())
                  for name,group in [('server',server),('nodes',nodes)]}
        result['gates']['no_child_cgroup_oom'] = all(int(e['oom'])==0 and int(e['oom_kill'])==0 for e in events.values())
        result['gates']['server_rss_le_1gib'] = result['server_peak_rss_mib'] <= 1024
        result['gates']['aggregate_nodes_rss_le_512mib'] = result['aggregate_nodes_peak_rss_mib'] <= 512
        result['gates']['no_child_swap'] = all((g/'memory.swap.current').read_text().strip() == '0' for g in (server,nodes))
        samples = json.loads((work/'resources.json').read_text())
        result['gates']['multiple_publications'] = max(r['segments'] for r in samples) >= 2
        result['gates']['drained_final_progress'] = all(p['pending_batches'] == 0 and p['file_backlog_bytes'] == 0 for p in samples[-1]['progress'])
        result['residency'] = residency(samples, observer.epoch)
        result['gates']['finite_residency_discriminator'] = result['residency']['h1_residency_compatible']
        result['passed'] = all(result['gates'].values())
        native.dump(work/'summary.json', result)
        status = 'passed' if result['passed'] else 'failed'
    except BaseException as exc:
        native.dump(out/'failure.json', dict(error=repr(exc)))
    finally:
        if work.exists():
            observer.archive(work)
            # Compact evidence is bounded before original success state removal.
            for p in sorted(work.iterdir()):
                if p.is_file() and p.name != 'recovered.jsonl' and p.suffix in ('.json','.gz','.out','.err'):
                    target = out/p.name
                    if p.suffix == '.gz':
                        # Normalize gzip timestamps for compact raw evidence.
                        with gzip.open(p,'rb') as src, target.open('wb') as raw, gzip.GzipFile(fileobj=raw,mode='wb',mtime=0) as dst:
                            shutil.copyfileobj(src,dst)
                    else:
                        shutil.copyfile(p,target)
                    if p.suffix == '.gz':
                        with gzip.open(p,'rb') as source, gzip.open(target,'rb') as copied:
                            if hashlib.file_digest(source,'sha256').hexdigest() != hashlib.file_digest(copied,'sha256').hexdigest():
                                raise RuntimeError('compact compressed evidence exact readback mismatch')
                    elif digest(p) != digest(target):
                        raise RuntimeError('compact evidence exact readback mismatch')
            compact = native.footprint(out)
            # Include final resource/cleanup receipts and their artifact map in
            # admission before deleting anything. These fixed-size receipts and
            # the bounded member map fit the reserved MiB for this fixture.
            bookkeeping = 1024**2
            if compact + bookkeeping > cap:
                status = 'failed'
                preservation = dict(removed=False,error='compact evidence exceeds preservation allowance',original=str(work))
            elif status == 'passed':
                shutil.rmtree(work)
                preservation = dict(removed=True,success_raw_evidence_bytes=compact)
            else:
                preservation = bounded_archive(work,out,cap-compact-bookkeeping,deadline)
            native.dump(out/'cleanup.json', dict(status=status, elapsed_s=time.monotonic()-started,
                logical_work_bytes=native.footprint(work) if work.exists() else 0, **preservation))
        native.dump(out/'cgroup-final.json',native.cgroups.snapshot(parent))
        # Empty owned children can be removed after retaining counters.
        for group in (server,nodes):
            if not (group/'cgroup.procs').read_text().strip():
                group.rmdir()
        # Origin: service-evidence-repair-proposal.md. Generate only after the
        # last cgroup-final write, otherwise the manifest is immediately stale.
        native.dump(out/'compact-sha256.json', {p.name:digest(p) for p in out.iterdir()
                    if p.is_file() and p.name != 'compact-sha256.json'})
        if native.footprint(out) > cap:
            raise RuntimeError('final receipts exceed preservation allowance')
    return 0 if status=='passed' else 1


if __name__=='__main__':
    raise SystemExit(main())
