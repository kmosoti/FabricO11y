#!/usr/bin/env python3
"""Run unchanged qualification fixtures from an owned data-drive bundle."""
import argparse,gzip,hashlib,json,os,shutil,subprocess,sys,time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[4]
sys.path.insert(0,str(ROOT/'tools'))
from resource_group import require_limits,STORAGE
DATA=ROOT/'docs/experiments/benchmarks/data/lab-completion-run-01'
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def main():
 require_limits()
 ap=argparse.ArgumentParser();ap.add_argument('--kind',choices=['soak','outage'],required=True);ap.add_argument('--seed',default='0xA11FA001');ap.add_argument('--smoke',action='store_true');a=ap.parse_args()
 work=Path(os.environ['FABRIC_SCRATCH_ROOT'])/'qualification';work.mkdir(exist_ok=False)
 if not work.resolve().is_relative_to(STORAGE/'scratch'):raise RuntimeError('data drive required')
 label=a.kind+'-'+a.seed+('-smoke' if a.smoke else '')
 out=DATA/'memory'/label;out.mkdir(parents=True,exist_ok=False)
 bundle=work/'bundle';(bundle/'tools').mkdir(parents=True);(bundle/'target').mkdir();(bundle/'bin/examples').mkdir(parents=True)
 shutil.copytree(ROOT/'tools/qualification',bundle/'tools/qualification',ignore=shutil.ignore_patterns('__pycache__'))
 bins=Path(os.environ['CARGO_TARGET_DIR'])/'release'
 for name in ['fabric-server','fabric-node','fabricctl','examples/server_dump','examples/spool_dump','examples/spindle_sim']:
  shutil.copy2(bins/name,bundle/'bin'/name)
 hashes={str(p.relative_to(bundle)):sha(p) for p in bundle.rglob('*') if p.is_file()}
 cpus=sorted(os.sched_getaffinity(0))
 if len(cpus)<4:raise RuntimeError('four allowed CPUs needed')
 trial=bundle/'target'/('alpha-'+label)
 seconds=7200 if a.kind=='soak' else 3000
 disk=5*2**30 if a.kind=='soak' else 2**30
 command=[sys.executable,str(bundle/'tools/qualification/runner.py'),'--out',str(trial),'--duration-s',str(seconds),'--disk-bytes',str(disk),'--max-output-bytes',str(2**20),'--',sys.executable,str(bundle/'tools/qualification'/('soak_tier.py' if a.kind=='soak' else 'outage_drain.py')),'--seed',a.seed,'--bin-dir',str(bundle/'bin')]
 if a.kind=='soak':command+=['--server-cpus',','.join(map(str,cpus[:2])),'--sim-cpus',','.join(map(str,cpus[2:4]))]
 if a.smoke:command+=(['--smoke'] if a.kind=='soak' else ['--outage-s','5'])
 (out/'environment.json').write_text(json.dumps({'command':command,'frozen_files':hashes,'cpus':cpus,'host':os.uname()._asdict() if hasattr(os.uname(),'_asdict') else list(os.uname()),'qualification':'local measured only; Fedora is not Debian13 target','smoke':a.smoke},indent=2))
 code=subprocess.call(command,cwd=ROOT)
 files={}
 # Preserve result and complete non-secret transcripts; owned state is reproducible
 # from frozen workload+seed, and remains on failure pending coordinator review.
 for p in trial.rglob('*'):
  relative=p.relative_to(trial)
  if p.is_file() and len(relative.parts)<=2 and (p.suffix in ('.json','.jsonl','.log','.err','.txt') or p.name in ('output','result')):
   dest=out/(str(relative)+'.gz');dest.parent.mkdir(parents=True,exist_ok=True)
   with p.open('rb') as src,gzip.open(dest,'wb',compresslevel=6) as dst:shutil.copyfileobj(src,dst)
   files[str(relative)]={'sha256':sha(p),'bytes':p.stat().st_size,'artifact':str(dest.relative_to(out))}
 total=sum(p.stat().st_size for p in (DATA/'memory').rglob('*') if p.is_file())
 if total>256*2**20:raise RuntimeError('capacity lab evidence exceeded256MiB')
 (out/'receipt.json').write_text(json.dumps({'exit':code,'files':files,'retained_lab_bytes':total,'scratch':str(work),'removed':code==0},indent=2))
 if code==0:shutil.rmtree(work)
 raise SystemExit(code)
if __name__=='__main__':main()
