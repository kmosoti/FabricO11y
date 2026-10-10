#!/usr/bin/env python3
"""Record a native RPM metadata build around exact isolated candidate Deb bytes."""
import argparse
import json
import os
import signal
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE.parent))
from resource_group import require_limits, STORAGE
from stage_candidate import digest, stage_candidate
from payload import assert_equivalent, inventory


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--deb-receipt', required=True, type=Path)
    parser.add_argument('--out', required=True, type=Path)
    args = parser.parse_args()
    require_limits()
    out = args.out.resolve()
    out.relative_to(STORAGE / 'results')
    out.mkdir(exist_ok=False, parents=True)
    source = json.loads(args.deb_receipt.read_text())
    package = Path(source['package'])
    with tempfile.TemporaryDirectory(dir=os.environ['FABRIC_SCRATCH_ROOT'], prefix='candidate-rpm-input-') as scratch:
        staged = Path(scratch) / 'validated'
        stage_candidate(package, args.deb_receipt, staged)
        provenance_hash = digest(staged / 'usr/share/doc/fabrico11y/CANDIDATE-PROVENANCE.json')
    epoch = source['source']['source_date_epoch']
    env = {**os.environ, 'SOURCE_DATE_EPOCH': str(epoch)}
    command = ['bash', str(ROOT / 'packaging/build-rpm.sh'), str(out), str(package), '--candidate-deb', str(args.deb_receipt)]
    record = {'state': 'failed', 'command': command, 'exit': 2,
              'deb_receipt': str(args.deb_receipt.resolve()), 'deb_receipt_sha256': digest(args.deb_receipt),
              'deb_sha256': source['package_sha256'], 'source': source['source'],
              'rpm_builder_sha256': digest(ROOT / 'packaging/build-rpm.sh'),
              'adapter_sha256': digest(Path(__file__)), 'package_family': 'fedora'}
    try:
        process = subprocess.Popen(command, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   text=True, start_new_session=True)
        try:
            stdout, stderr = process.communicate(timeout=300)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                stdout, stderr = process.communicate(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                stdout, stderr = process.communicate(timeout=10)
            (out / 'build.stdout').write_text(stdout)
            (out / 'build.stderr').write_text(stderr)
            raise ValueError('RPM build deadline exceeded; owned process group stopped')
        (out / 'build.stdout').write_text(stdout)
        (out / 'build.stderr').write_text(stderr)
        record['build_exit'] = process.returncode
        if process.returncode:
            raise ValueError('RPM build failed')
        packages = list(out.glob('*.rpm'))
        if len(packages) != 1:
            raise ValueError('expected exactly one candidate RPM')
        payload = assert_equivalent(package, packages[0])
        if inventory(packages[0])['usr/share/doc/fabrico11y/CANDIDATE-PROVENANCE.json']['sha256'] != provenance_hash:
            raise ValueError('RPM candidate provenance differs from exact validated build inputs')
        record.update(state='built', package=str(packages[0]), package_sha256=digest(packages[0]),
                      payload_identity=payload, candidate_provenance_sha256=provenance_hash, exit=0)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        record.update(error=str(error), exit=2)
    (out / 'receipt.json').write_text(json.dumps(record, indent=2) + '\n')
    print(json.dumps(record, indent=2))
    return record['exit']


if __name__ == '__main__':
    raise SystemExit(main())
