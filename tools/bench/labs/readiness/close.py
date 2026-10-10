#!/usr/bin/env python3
"""Use the existing documentation runtime and close reviewed verification scratch."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[4]
DATA = ROOT / 'docs/experiments/benchmarks/data/readiness-labs-run-01'


def main():
    out = DATA / 'verification-close'
    out.mkdir(exist_ok=False)
    source = Path('/tmp/fabric-bun-runtime/bun-linux-x64/bun')
    scratch = Path(os.environ['FABRIC_LAB_SCRATCH']).resolve()
    if not scratch.is_relative_to(Path('/run/media/kmosoti/data/FabricO11y/scratch')):
        raise RuntimeError('data-drive scratch required')
    binary = scratch / 'bun'
    shutil.copy2(source, binary)
    with binary.open('rb') as stream:
        checksum = hashlib.file_digest(stream, 'sha256').hexdigest()
    version = subprocess.check_output([str(binary), '--version'], text=True).strip()
    command = [str(binary), 'tools/docs/check.mjs']
    with (out / 'documentation.txt').open('w') as stream:
        child = subprocess.run(command, cwd=ROOT, stdout=stream, stderr=subprocess.STDOUT)
    (out / 'command.json').write_text(json.dumps({'command': command, 'exit': child.returncode,
        'runtime_source': str(source), 'runtime_sha256': checksum, 'runtime_version': version}, indent=2) + '\n')
    if child.returncode:
        raise SystemExit(child.returncode)
    cleanup = []
    for job in ('v1-checks', 'v2-check-repair'):
        record = json.loads((DATA / 'coordinator' / job / 'receipt.json').read_text())
        unit = Path(record['cgroup']).name.removesuffix('.service')
        receipt = ROOT / 'target/resource-containment/runs' / (unit + '.json')
        launcher = json.loads(receipt.read_text())
        shutil.copyfile(receipt, DATA / 'launcher-receipts' / receipt.name)
        owned = Path('/run/media/kmosoti/data/FabricO11y/evidence') / unit
        if record['exit'] != 1 or launcher['retained_failure_evidence'] != str(owned) or owned.is_symlink():
            raise RuntimeError('verification scratch ownership differs')
        size = sum(p.stat().st_size for p in owned.rglob('*') if p.is_file())
        shutil.rmtree(owned)
        cleanup.append({'job': job, 'owned_path': str(owned), 'removed': not owned.exists(),
                        'logical_bytes_removed': size, 'failed_receipt_preserved': True})
    binary.unlink()
    (out / 'cleanup.json').write_text(json.dumps({'verification_scratch': cleanup,
        'copied_runtime_removed': not binary.exists()}, indent=2) + '\n')
    sizes = {lab: sum(p.stat().st_size for p in (DATA / lab).rglob('*') if p.is_file())
             for lab in ('memory', 'query', 'recovery', 'coordinator')}
    (out / 'retained-evidence-bytes.json').write_text(json.dumps(sizes, indent=2) + '\n')
    if any(size > 50 * 1024**2 for size in sizes.values()):
        raise RuntimeError('compact lab evidence exceeds its bound')
    print(json.dumps({'documentation_exit': 0, 'runtime_version': version,
                      'reviewed_verification_scratch_removed': True, 'evidence_bytes': sizes}))


if __name__ == '__main__':
    main()
