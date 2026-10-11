"""Accept only a real missing-ACKed-record negative control, never a startup error."""
import json
import os
from pathlib import Path
import subprocess
import sys


def detected(status, summary):
    return (status == 1 and summary.get('mutate') == 'drop-recovered'
            and summary.get('passed') is False and summary.get('oracle_passed') is False
            and summary.get('companion_observed') is True
            and summary.get('server_exit') == 0
            and summary.get('node_exits') == [0, 0, 0]
            and summary.get('acks', 0) > 0 and summary.get('recovered', 0) > 0
            and {v.get('rule') for v in summary.get('violations', [])} == {'ACKED-DURABLE'})


if __name__ == '__main__':
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
    from resource_group import require_limits
    require_limits()
    command = [sys.executable, '-B', str(Path(__file__).with_name('delivery_faults.py')),
               '--scenario', 'clean', '--seed', '3', '--seconds', '8',
               '--mutate', 'drop-recovered']
    status = subprocess.run(command).returncode
    summary_path = Path(os.environ['FABRIC_SCRATCH_ROOT']) / 'alpha-delivery-clean-3/summary.json'
    summary = json.loads(summary_path.read_text())
    if not detected(status, summary):
        raise SystemExit('delivery control did not reach the registered ACKED-DURABLE defect')
    print('registered missing-ACKed-record defect detected')
