"""Archive and byte-verify owned failures before removing their scratch."""
import argparse,hashlib,json,shutil,subprocess,sys,tarfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[4]
sys.path.insert(0,str(ROOT/'tools'))
from resource_group import require_limits,STORAGE
DATA=ROOT/'docs/experiments/benchmarks/data/lab-completion-run-01'
def sha(stream):return hashlib.file_digest(stream,'sha256').hexdigest()
def main():
 require_limits();ap=argparse.ArgumentParser();ap.add_argument('--id',required=True);ap.add_argument('--remove-sdk',action='store_true');ap.add_argument('--only-job',action='append');a=ap.parse_args()
 copies=DATA/'coordinator/launcher-receipts';copies.mkdir(exist_ok=True);rows=[]
 for path in sorted((DATA/'coordinator').glob('*/receipt.json')):
  r=json.loads(path.read_text())
  if a.only_job and r['id'] not in a.only_job:continue
  if r['state']=='running':continue
  unit=r['cgroup'].rsplit('/',1)[-1].removesuffix('.service')
  original=ROOT/'target/resource-containment/runs'/(unit+'.json')
  if not original.exists():raise RuntimeError('outer receipt missing: '+unit)
  outer=json.loads(original.read_text());shutil.copyfile(original,copies/(r['id']+'.json'))
  status=subprocess.run(['systemctl','--user','show',unit+'.service','--property=ActiveState','--value'],capture_output=True,text=True).stdout.strip()
  if status not in ('','inactive','failed'):raise RuntimeError('completed service active')
  row={'id':r['id'],'state':status,'exit':r['exit'],'removed_bytes':0}
  if outer['retained_failure_evidence']:
   retained=Path(outer['retained_failure_evidence'])
   if retained.parent!=STORAGE/'evidence' or retained.name!=unit or retained.is_symlink():raise RuntimeError('failure ownership mismatch')
   if retained.exists():
    destination=DATA/r['lab']/('failure-'+r['id']+'.tar.gz');destination.parent.mkdir(exist_ok=True)
    manifest={}
    for p in retained.rglob('*'):
     if p.is_symlink():raise RuntimeError('linked failure scratch requires separate review')
     if p.is_file():
      with p.open('rb') as f:manifest[str(p.relative_to(retained))]={'sha256':sha(f),'bytes':p.stat().st_size}
    if not destination.exists():
     with tarfile.open(destination,'x:gz') as archive:
      for name in manifest:archive.add(retained/name,arcname=name,recursive=False)
    # Read every retained byte back before destroying the original copy.
    with tarfile.open(destination,'r:gz') as archive:
     members=archive.getmembers()
     if set(m.name for m in members)!=set(manifest):raise RuntimeError('archive file coverage mismatch')
     for m in members:
      if not m.isfile() or m.size!=manifest[m.name]['bytes'] or sha(archive.extractfile(m))!=manifest[m.name]['sha256']:
       raise RuntimeError('failure archive differs from original bytes')
    if sum(p.stat().st_size for p in destination.parent.rglob('*') if p.is_file())>256*2**20:
     raise RuntimeError('lab evidence budget exceeded; retain original')
    destination.with_suffix('.manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
    row.update(removed_bytes=sum(x['bytes'] for x in manifest.values()),archive=str(destination.relative_to(DATA)),byte_verified=True)
    shutil.rmtree(retained)
   row['scratch_removed']=not retained.exists()
  rows.append(row)
 if a.remove_sdk:
  venv=STORAGE/'sdk-completion-1.38.0'
  if venv.is_symlink():raise RuntimeError('unexpected linked SDK cache')
  if venv.exists():shutil.rmtree(venv)
 sizes={lab:sum(p.stat().st_size for p in (DATA/lab).rglob('*') if p.is_file()) for lab in ('memory','query','recovery','coordinator')}
 if any(n>256*2**20 for n in sizes.values()) or sum(sizes.values())>2*2**30:raise RuntimeError('retained budget exceeded')
 (DATA/(a.id+'.json')).write_text(json.dumps({'jobs':rows,'lab_bytes':sizes,'sdk_removed':a.remove_sdk,
   'shared_build_cache':'preserved','limitation':'current cleanup and outer launcher receipts follow this report'},indent=2)+'\n')
 print(json.dumps({'jobs':len(rows),'bytes_removed':sum(r['removed_bytes'] for r in rows),'lab_bytes':sizes}))
if __name__=='__main__':main()
