"""Finite owned builder syscalls; failed injection is never a passing fault."""
import argparse,hashlib,json,os,shutil,subprocess,sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[4]
sys.path.insert(0,str(ROOT/'tools'))
from resource_group import require_limits
def dump(p,v):p.write_text(json.dumps(v,indent=2)+'\n')
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def leftovers(state):
 return sorted(str(p.relative_to(state)) for p in state.rglob('*')
               if (p.is_dir() and p.name.startswith('.building'))
               or (p.is_file() and p.name.startswith('.run-')))
def grade(receipt, killed):
 return (receipt['injected'] and receipt['failed_exit']!=0
         and (not killed or receipt['failed_exit']==-9)
         and receipt['input_unchanged'] and receipt['retry_exit']==0
         and receipt['exact_manifest'] and not receipt['leftovers']
         and (killed or not receipt['pre_restart_leftovers']))
def main():
 require_limits()
 ap=argparse.ArgumentParser();ap.add_argument('--id',default='syscall-01');a=ap.parse_args()
 out=ROOT/'docs/experiments/benchmarks/data/lab-completion-run-01/recovery'/a.id;out.mkdir(parents=True,exist_ok=False)
 work=Path(os.environ['FABRIC_SCRATCH_ROOT'])/'faults';work.mkdir()
 bins=Path(os.environ['CARGO_TARGET_DIR'])/'release/examples'; helper=bins/'completion_fault';gen=bins/'completion_builder'
 library=work/'fault.so'
 subprocess.run(['gcc','-shared','-fPIC','-O2','-Wall','-o',str(library),str(Path(__file__).with_name('io_fault.c')),'-ldl'],check=True)
 result=subprocess.run([gen,'gen',work/'input','steady','16'],capture_output=True,text=True,check=True,timeout=120)
 dump(out/'fixture.json',json.loads(result.stdout)); source=Path(json.loads(result.stdout)['input']);original=sha(source)
 def run(label,state,mode,env):
  r=subprocess.run([helper,state,source,mode],env=env,capture_output=True,text=True,timeout=120)
  (out/(label+'.stdout')).write_text(r.stdout);(out/(label+'.stderr')).write_text(r.stderr)
  dump(out/(label+'.command.json'),{'exit':r.returncode,'mode':mode,'state':str(state),'fault_env':{k:v for k,v in env.items() if k.startswith('FABRIC_FAULT_')}})
  return r
 clean=dict(os.environ); reference=run('control',work/'control','build',clean)
 if reference.returncode:raise RuntimeError('clean fixture failed')
 expected=json.loads(reference.stdout)
 # Negative control: a deliberately nonexistent path must not claim injection.
 miss=dict(clean,LD_PRELOAD=str(library),FABRIC_FAULT_ROOT=str(work),FABRIC_FAULT_MATCH='DOES-NOT-EXIST',FABRIC_FAULT_OP='write')
 negative=run('no-hit-control',work/'no-hit','build',miss)
 if negative.returncode or 'FABRIC_INJECTION' in negative.stderr:raise RuntimeError('no-hit control incorrect')
 cases=[('spill-write','write','.run-0-',False),('merge-read','read','.run-0-',False),
        ('raw-write','write','batches.parquet',False),('logs-write','write','logs.parquet',False),
        ('metrics-write','write','metrics.parquet',False),('filter-write','write','text_filter.bin',False),
        ('manifest-write','write','manifest.json',False),('table-sync','sync','logs.parquet',False),
        ('manifest-sync','sync','manifest.json',False),('publication','rename','.building-00000000000000000001$',False),
        ('published-dir-sync','sync','/segments$',False),('kill-spill','write','.run-0-',True),
        ('kill-manifest','sync','manifest.json',True),('kill-before-publication','rename','.building-00000000000000000001$',True),
        ('journal-read','read','sealed-',False),('partial-spill-write','write','.run-0-',False),
        ('partial-merge-read','read','.run-0-',False)]
 outcomes=[]
 for label,op,pattern,kill in cases:
  state=work/label;env=dict(clean,LD_PRELOAD=str(library),FABRIC_FAULT_ROOT=str(work),FABRIC_FAULT_MATCH=pattern,FABRIC_FAULT_OP=op)
  if kill:env['FABRIC_FAULT_KILL']='1'
  if label.startswith('partial-'):env['FABRIC_FAULT_PARTIAL']='1'
  failed=run(label,state,'build',env)
  hit='FABRIC_INJECTION' in failed.stderr
  receipt={'case':label,'injected':hit,'failed_exit':failed.returncode,'input_unchanged':sha(source)==original,
           'pre_restart_files':[str(p.relative_to(state)) for p in state.rglob('*') if p.is_file()],
           'pre_restart_leftovers':leftovers(state)}
  # Ordinary errors must clean before retry. Recovery cannot erase their evidence.
  dump(out/(label+'.before-recovery.json'),receipt)
  recovered=run(label+'-retry',state,'recover',clean)
  receipt.update(retry_exit=recovered.returncode,exact_manifest=recovered.returncode==0 and json.loads(recovered.stdout)==expected,
                 leftovers=leftovers(state))
  receipt['passed']=grade(receipt,kill)
  outcomes.append(receipt);dump(out/'outcomes.json',outcomes)
 if not all(x['passed'] for x in outcomes):raise RuntimeError('one or more syscall faults failed; preserve state')
 shutil.rmtree(work);dump(out/'cleanup.json',{'removed':True,'scratch':str(work)})
if __name__=='__main__':main()
