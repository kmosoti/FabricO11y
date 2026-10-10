"""Run registered verification profiles and preserve their exact receipts."""
import argparse,json,os,shutil,subprocess,sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[4]
sys.path.insert(0,str(ROOT/'tools'))
from resource_group import require_limits
def main():
 require_limits();ap=argparse.ArgumentParser();ap.add_argument('--profile',choices=['fast','extended','documentation'],required=True);ap.add_argument('--id',required=True);a=ap.parse_args()
 out=ROOT/'docs/experiments/benchmarks/data/lab-completion-run-01/recovery'/a.id;out.mkdir(parents=True,exist_ok=False)
 env=dict(os.environ)
 if a.profile=='documentation':
  source=Path('/tmp/fabric-bun-runtime/bun-linux-x64/bun')
  if source.is_file():
   target=Path(os.environ['TMPDIR'])/'bun';shutil.copy2(source,target);env['PATH']=str(target.parent)+':'+env['PATH']
 command=['cargo','xtask','checks','--profile',a.profile]
 code=subprocess.call(command,env=env)
 registered=json.loads((ROOT/'xtask/checks.json').read_text())['checks']
 for check in registered:
  if check['profile']==a.profile:
   receipt=ROOT/'target/verification/receipts'/(check['id']+'.json')
   if receipt.exists():shutil.copyfile(receipt,out/receipt.name)
 (out/'command.json').write_text(json.dumps({'command':command,'exit':code},indent=2)+'\n')
 raise SystemExit(code)
if __name__=='__main__':main()
