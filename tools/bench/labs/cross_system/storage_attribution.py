#!/usr/bin/env python3
"""Small observer/plain phase diagnostic, existing semantic checker unchanged."""
import argparse
import gzip
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

import memory_census as mc
import storage_sweep as sweep

PROBE = mc.ROOT/'crates/fabric-server/examples/storage_attribution_probe.rs'
LIMIT = 32*mc.MIB


def main():
    mc.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--destination', type=Path, required=True)
    parser.add_argument('--protocol', type=Path, required=True)
    args = parser.parse_args()
    destination = args.destination.resolve()
    if destination.exists() or not destination.is_relative_to(sweep.REGISTERED.resolve()):
        raise ValueError('fresh registered memory evidence directory required')
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    if not scratch.is_relative_to(mc.DATA.resolve()):
        raise ValueError('data-drive scratch required')
    destination.mkdir(parents=True)
    for path, name in [(PROBE,'probe.rs'),(Path(__file__),'runner.py'),(args.protocol,'protocol.txt')]:
        shutil.copyfile(path,destination/name)
    mc.dump(destination/'negative-controls.json',mc.controls())
    deadline = time.monotonic()+540
    work = Path(tempfile.mkdtemp(prefix='storage-attribution-',dir=scratch))
    decoder = work/'decoder'
    decoder.mkdir()
    original_pythonpath = os.environ.get('PYTHONPATH')
    original_observe = os.environ.get('FABRIC_STORAGE_ATTRIBUTION_OBSERVE')
    try:
        install = ['uv','pip','install','--no-cache','--only-binary',':all:', '--python',sys.executable,
                   '--target',str(decoder),'pyarrow==22.0.0']
        result = subprocess.run(install,timeout=45)
        mc.dump(destination/'decoder-install.json',{'argv':install,'exit_code':result.returncode,
                'package_metadata_sha256': {str(path.relative_to(decoder)):mc.digest(path)
                      for path in decoder.glob('*.dist-info/*') if path.is_file()}})
        result.check_returncode()
        os.environ['PYTHONPATH'] = str(decoder)
        provider = mc.decoder_call('version',deadline)
        binaries = {}
        build_env = dict(os.environ, CARGO_BUILD_JOBS='2')
        for name in sweep.EXPERIMENT_ENV:
            build_env.pop(name,None)
        for cap in (8,16):
            build_env['FABRIC_RUN_MIB_EXPERIMENT'] = str(cap)
            argv = ['cargo','build','--offline','--locked','--release','-p','fabric-server',
                    '--example','storage_attribution_probe','--features','phase-probe']
            with (destination/f'build-{cap}.log').open('wb') as log:
                result = subprocess.run(argv,cwd=mc.ROOT,env=build_env,stdout=log,
                                        stderr=subprocess.STDOUT,timeout=120)
            mc.dump(destination/f'build-{cap}.json',{'argv':argv,'exit_code':result.returncode,
                    'features':['phase-probe'],'compile_run_mib':cap,'cargo_build_jobs':2})
            result.check_returncode()
            binary = work/f'probe-{cap}'
            shutil.copyfile(Path(os.environ['CARGO_TARGET_DIR'])/'release/examples/storage_attribution_probe',binary)
            binary.chmod(0o755)
            binaries[cap]=binary
        mc.dump(destination/'metadata.json',{'probe_sha256':mc.digest(PROBE),'runner_sha256':mc.digest(Path(__file__)),
                'protocol_sha256':mc.digest(args.protocol),'seed':2703204353,**provider,
                'binaries':{str(cap):{'sha256':mc.digest(path),'path':str(path)} for cap,path in binaries.items()},
                'qualification':False,'observer_ledger_capacity':262144,'trace_envelope_records':16384,
                'sample_slots':['CPU unsupported=0','live heap','global high-water heap','requested unsupported=0'],
                'interpretation':'inclusive/global snapshots; overlaps and uninstrumented allocation gaps remain'})
        pairs=[]
        fixtures = work/'fixtures'
        fixtures.mkdir()
        for target in (16,64):
            for cap in (8,16):
                if sweep.footprint(destination)+16*mc.MIB > LIMIT:
                    raise RuntimeError('16MiB failure reserve unavailable within32MiB diagnostic cap')
                pair_dir=destination/f'target-{target}-run-{cap}'
                pair_dir.mkdir()
                order=['plain','observed'] if (target==16) == (cap==8) else ['observed','plain']
                rows={}
                states=[]
                for arm in order:
                    cell=pair_dir/arm
                    cell.mkdir()
                    state=fixtures/f'target-{target}-run-{cap}-{arm}'
                    states.append(state)
                    os.environ['FABRIC_STORAGE_ATTRIBUTION_OBSERVE']='1' if arm=='observed' else '0'
                    rows[arm]=sweep.native(binaries[cap],state,cell,'steady',target,2703204353,
                                           'bounded',mc.cgroup(),work,destination,deadline)
                    row=rows[arm]['probe']
                    if row['phase_probe_observed'] != (arm=='observed'):
                        raise ValueError('observer flag differs from requested arm')
                    if arm=='observed':
                        trace=state/'phase-events.jsonl'
                        events=[json.loads(line) for line in trace.read_text().splitlines()]
                        if len(events)!=row['phase_records'] or len(events)>16384:
                            raise ValueError('phase ledger count/envelope mismatch')
                        baseline=row['heap_start_bytes']
                        by_name={}
                        new_peak_intervals=[]
                        for event in events:
                            entry=by_name.setdefault(event['name'],{'count':0,'maximum_live_before_minus_baseline':0,
                                'maximum_live_after_minus_baseline':0,'maximum_global_peak_minus_baseline':0,
                                'inclusive_wall_ns':0})
                            entry['count']+=1
                            entry['inclusive_wall_ns']+=event['wall_ns']
                            entry['maximum_live_before_minus_baseline']=max(entry['maximum_live_before_minus_baseline'],event['before'][1]-baseline)
                            entry['maximum_live_after_minus_baseline']=max(entry['maximum_live_after_minus_baseline'],event['after'][1]-baseline)
                            entry['maximum_global_peak_minus_baseline']=max(entry['maximum_global_peak_minus_baseline'],event['after'][2]-baseline)
                            if event['after'][2]>event['before'][2]:
                                new_peak_intervals.append({'name':event['name'],'thread':event['thread'],
                                    'depth':event['depth'],'start_ns':event['start_ns'],
                                    'end_ns':event['start_ns']+event['wall_ns'],
                                    'before_peak_minus_baseline':event['before'][2]-baseline,
                                    'after_peak_minus_baseline':event['after'][2]-baseline,
                                    'interpretation':'enclosing interval, nested/global observations; not exclusive ownership'})
                        with gzip.open(cell/'phase-events.jsonl.gz','wb') as output:
                            output.write(trace.read_bytes())
                        with gzip.open(cell/'phase-events.jsonl.gz','rb') as packed:
                            if packed.read()!=trace.read_bytes():
                                raise ValueError('phase trace exact readback mismatch')
                        mc.dump(cell/'phase-summary.json',{'by_name':by_name,'heap_start_bytes':baseline,
                                'observer_live_bytes':row['observer_live_bytes'],'events':len(events),
                                'new_peak_enclosing_intervals':new_peak_intervals,
                                'inclusive_wall_sums_not_additive':True,
                                'peak_stage_ownership':'not established by inclusive/global snapshots alone'})
                gates=mc.grade(rows['plain']['probe'],rows['observed']['probe'])
                pair={'target_mib':target,'run_mib':cap,'order':order,'rows':rows,'gates':gates}
                mc.dump(pair_dir/'pair.json',pair)
                pairs.append(pair)
                if not all(gates.values()):
                    raise ValueError('unchanged observer/plain semantic gate failure')
                for state in states:
                    shutil.rmtree(state)
                mc.dump(pair_dir/'cleanup.json',{'removed':all(not state.exists() for state in states)})
        mc.dump(destination/'summary.json',{'pairs':pairs,'qualification':False})
        shutil.rmtree(work)
        mc.dump(destination/'complete.json',{'exit_code':0,'native_cells':8,'scratch_removed':not work.exists(),
                'retained_bytes':sweep.footprint(destination),'qualification':False})
    except BaseException as error:
        # Never archive decoder/tools as if they were a semantic fixture.
        if decoder.exists():
            shutil.rmtree(decoder)
        if sweep.footprint(destination)>=LIMIT:
            mc.dump(destination/'failure.json',{'error':repr(error),'retained_scratch':str(work),'scratch_removed':False})
        else:
            old_limit,old_failure=sweep.EVIDENCE_LIMIT,sweep.FAILURE_LIMIT
            try:
                # Admission budget is local to this separate diagnostic, while
                # the coordinator independently enforces aggregate memory256MiB.
                sweep.EVIDENCE_LIMIT=sweep.footprint(sweep.REGISTERED)+LIMIT-sweep.footprint(destination)
                sweep.FAILURE_LIMIT=16*mc.MIB
                sweep.preserve_failure(work,destination,error)
            finally:
                sweep.EVIDENCE_LIMIT,sweep.FAILURE_LIMIT=old_limit,old_failure
        raise
    finally:
        for key,value in [('PYTHONPATH',original_pythonpath),('FABRIC_STORAGE_ATTRIBUTION_OBSERVE',original_observe)]:
            if value is None:
                os.environ.pop(key,None)
            else:
                os.environ[key]=value


if __name__=='__main__':
    main()
