"""Serial archive and unchanged fast-profile retry after scoped corrections."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
sys.path.insert(0,str(Path(__file__).resolve().parents[3]))
from resource_group import require_limits
require_limits()
old=Path('docs/experiments/benchmarks/data/lab-completion-run-01/coordinator/catalog-coupled-completion-fast-01/verification-receipts')
old.mkdir(exist_ok=False)
manifest={}
for source in sorted(Path('target/verification/receipts').glob('*.json')):
    data=source.read_bytes();target=old/source.name
    target.write_bytes(data)
    if target.read_bytes()!=data:raise RuntimeError('receipt copy mismatch')
    manifest[source.name]={'sha256':hashlib.sha256(data).hexdigest(),'bytes':len(data)}
(old/'copy-manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
commands=[
    [sys.executable,'-B','tools/bench/labs/catalog/coupled_cleanup.py','--id','catalog-coupled-failure-cleanup-06','--only-job','catalog-coupled-completion-fast-01'],
    ['cargo','fmt','--all'],
    ['cargo','xtask','checks','--profile','fast'],
]
for command in commands:
    print(json.dumps({'command':command}),flush=True)
    result=subprocess.run(command)
    print(json.dumps({'command':command,'exit':result.returncode}),flush=True)
    if result.returncode:raise SystemExit(result.returncode)
