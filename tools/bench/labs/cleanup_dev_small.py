#!/usr/bin/env python3
"""Archive launcher receipts and remove reviewed, owned preparation scratch."""
import json
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import require_limits, STORAGE

DATA = ROOT / 'docs/experiments/benchmarks/data/dev-small-labs-run-02'


def main():
    require_limits()
    copies = DATA / 'coordinator' / 'launcher-receipts'
    copies.mkdir(exist_ok=True)
    verification = DATA / 'recovery' / 'fast-receipts'
    verification.mkdir(parents=True, exist_ok=True)
    checks = json.loads((ROOT / 'xtask/checks.json').read_text())['checks']
    for check in checks:
        if check['profile'] == 'fast':
            name = check['id'] + '.json'
            shutil.copyfile(ROOT / 'target/verification/receipts' / name, verification / name)
    receipts = []
    for path in sorted((DATA / 'coordinator').glob('*/receipt.json')):
        record = json.loads(path.read_text())
        if record['state'] == 'running':
            continue
        unit = record['cgroup'].rsplit('/', 1)[-1].removesuffix('.service')
        original = ROOT / 'target/resource-containment/runs' / (unit + '.json')
        saved = copies / (record['id'] + '.json')
        if not saved.exists():
            shutil.copyfile(original, saved)
        current = subprocess.run(['systemctl','--user','show',unit+'.service',
            '--property=ActiveState','--value'], capture_output=True, text=True)
        state = current.stdout.strip()
        if state not in ('', 'inactive', 'failed'):
            raise RuntimeError('completed experiment service still active: ' + unit)
        receipts.append({'id':record['id'], 'unit':unit, 'state':state,
                         'launcher_receipt':str(saved.relative_to(DATA))})
    # Sole preparation failure: compiler diagnostic/source were retained in
    # the coordinator record; the repair and full fixture controls passed.
    original = json.loads((copies / 'prep-build.json').read_text())
    expected = STORAGE / 'evidence' / 'fabric-work-22b81e8a827f4ad3801109fdd01b82d4'
    retained = Path(original['retained_failure_evidence'])
    if retained != expected or retained.is_symlink():
        raise RuntimeError('unexpected failure scratch ownership')
    job = DATA / 'coordinator/prep-build'
    if not all((job / name).is_file() for name in ('source.tar.gz','stderr.txt','receipt.json')):
        raise RuntimeError('failure provenance missing')
    files = [str(path.relative_to(retained)) for path in retained.rglob('*') if path.is_file()]
    size = sum(path.stat().st_size for path in retained.rglob('*') if path.is_file())
    if retained.exists():
        shutil.rmtree(retained)
    result = {'completed_services':receipts, 'reviewed_failure_cleanup':{
        'path':str(retained), 'bytes':size, 'files':files, 'removed':not retained.exists(),
        'reason':'Private KeyValue initializer compile failure; original diagnostic and source retained, repair and fixture controls exited 0.'},
        'shared_caches':'preserved', 'remote_workloads':'none',
        'remaining_uncertainty':'This job and its outer launcher are still running while this receipt is written; their own exit/cleanup receipt follows.'}
    with (DATA / 'cleanup-review.json').open('x') as stream:
        json.dump(result, stream, indent=2)
        stream.write('\n')
    print(json.dumps({'reviewed_failure_removed':not retained.exists(), 'completed_services':len(receipts)}))


if __name__ == '__main__':
    main()
