"""Supplemental logical spill-write accounting; not a timing comparison."""
import argparse,hashlib,json,os,re,shutil,subprocess,sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[4]
sys.path.insert(0,str(ROOT/'tools'))
from resource_group import require_limits
CELLS=['steady-16','steady-32','steady-64','steady-128','steady-256','outage-64','adversarial-64','bigrows-64']
def dump(p,x):p.write_text(json.dumps(x,indent=2)+'\n')
def count(stderr):
 m=re.findall(r'FABRIC_SPILL bytes=(\d+) calls=(\d+)',stderr)
 if len(m)!=1:raise RuntimeError('exactly one accounting receipt required')
 return {'logical_bytes':int(m[0][0]),'successful_syscalls':int(m[0][1])}
def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def frozen_child(binary,expected,command,**options):
 if sha(binary)!=expected:raise RuntimeError('frozen builder binary changed before child')
 try:return subprocess.run(command,**options)
 finally:
  if sha(binary)!=expected:raise RuntimeError('frozen builder binary changed during child')
def measurement_env(library,work):
 env={k:v for k,v in os.environ.items() if not k.startswith('FABRIC_FAULT_')}
 env.update(LD_PRELOAD=str(library),FABRIC_MEASURE_ROOT=str(work))
 return env
def main():
 require_limits()
 ap=argparse.ArgumentParser();ap.add_argument('--id',default='spill-01')
 ap.add_argument('--campaign-root',type=Path,default=ROOT/'docs/experiments/benchmarks/data/lab-completion-run-01/memory')
 ap.add_argument('--binary',type=Path);ap.add_argument('--out',type=Path);a=ap.parse_args()
 data=a.campaign_root
 out=a.out or data/a.id;out.mkdir(parents=True,exist_ok=False)
 binary=a.binary or Path(os.environ['CARGO_TARGET_DIR'])/'release/examples/completion_builder'
 binary_sha256=sha(binary)
 comparisons=[]
 for cell in CELLS:
  provenance=data/('builder-'+cell)/'provenance.json'
  try:
   expected=json.loads(provenance.read_text())['hashes']['binary']
   comparisons.append({'cell':cell,'provenance':str(provenance),'expected_binary_sha256':expected,
      'matches':expected==binary_sha256})
  except (OSError,ValueError,KeyError,TypeError) as error:
   comparisons.append({'cell':cell,'provenance':str(provenance),'matches':False,'error':repr(error)})
 dump(out/'provenance.json',{'campaign_root':str(data),'binary':str(binary),'binary_sha256':binary_sha256,'comparisons':comparisons,
   'command':sys.argv,'supplemental_replays_per_cell':1,'timing_comparison':False,'heap_comparison':False})
 if not all(row['matches'] for row in comparisons):raise RuntimeError('selected binary does not match every original builder provenance')
 work=Path(os.environ['FABRIC_SCRATCH_ROOT'])/'spill-accounting';work.mkdir()
 lib=work/'observe.so';source=Path(__file__).with_name('io_fault.c')
 subprocess.run(['gcc','-shared','-fPIC','-O2','-o',lib,source,'-ldl'],check=True)
 observer_sha256=sha(lib);observer_source_sha256=sha(source)
 dump(out/'observer.json',{'library':str(lib),'library_sha256':observer_sha256,'source_sha256':observer_source_sha256})
 # Two different syscall entry points; a non-run output must not be counted.
 control=work/'control.c';control.write_text('#include <fcntl.h>\n#include <unistd.h>\n#include <sys/uio.h>\nint main(int n,char**v){int f=open(v[1],O_CREAT|O_WRONLY,0600);write(f,"abc",3);struct iovec a[2]={{"defg",4},{"hi",2}};writev(f,a,2);close(f);f=open(v[2],O_CREAT|O_WRONLY,0600);write(f,"excluded",8);close(f);return 0;}\n')
 subprocess.run(['gcc','-o',work/'control',control],check=True)
 env=measurement_env(lib,work)
 r=subprocess.run([work/'control',work/'logs.run-0-0',work/'excluded'],env=env,capture_output=True,text=True,check=True)
 observed=count(r.stderr)
 if observed!={'logical_bytes':9,'successful_syscalls':2}:raise RuntimeError('spill observer failed exact control')
 dump(out/'control.json',observed)
 for cell in CELLS:
  shape,mib=cell.rsplit('-',1);root=work/cell;root.mkdir()
  generation=[binary,'gen',root/'input',shape,mib]
  r=frozen_child(binary,binary_sha256,generation,capture_output=True,text=True,check=True,timeout=150)
  fixture=json.loads(r.stdout)
  dump(out/(cell+'.fixture.json'),{'argv':[str(x) for x in generation],'exit':r.returncode,'fixture':fixture})
  command=[binary,'run',root/'state',fixture['input'],'bounded']
  if sha(lib)!=observer_sha256:raise RuntimeError('frozen observer changed before child')
  p=frozen_child(binary,binary_sha256,command,env=env,capture_output=True,text=True,timeout=120)
  if sha(lib)!=observer_sha256:raise RuntimeError('frozen observer changed during child')
  (out/(cell+'.stderr')).write_text(p.stderr)
  (out/(cell+'.stdout.json')).write_text(p.stdout)
  if p.returncode:raise RuntimeError('instrumented builder failed')
  row=json.loads(p.stdout)
  prior=json.loads((data/('builder-'+cell)/'pair-1.json').read_text())['bounded']
  equal=row['manifest']==prior['manifest'] and row['input_sha256']==prior['input_sha256']
  spilled=count(p.stderr)
  if not spilled['logical_bytes'] or not spilled['successful_syscalls']:raise RuntimeError('nonempty bounded fixture produced no observed spill writes')
  dump(out/(cell+'.json'),{'cell':cell,'spill':spilled,'manifest_identical':equal,
     'binary_sha256':sha(binary),'observer_sha256':observer_source_sha256,'observer_library_sha256':observer_sha256,
     'input_sha256':row['input_sha256'],'argv':[str(x) for x in command],'exit':p.returncode,'timing_comparison':False,'heap_comparison':False})
  if not equal:raise RuntimeError('observer changed fixture or final manifest')
  shutil.rmtree(root)
 shutil.rmtree(work);dump(out/'cleanup.json',{'removed':True})
if __name__=='__main__':main()
