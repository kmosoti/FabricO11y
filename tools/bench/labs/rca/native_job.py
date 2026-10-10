"""Native RCA pilot charged to the existing pressure allocation."""
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools/bench/labs/hammer'))
import job

job.base.PROTOCOL = ROOT / 'docs/experiments/benchmarks/rca-native-pilot-protocol.md'

if __name__ == '__main__':
    raise SystemExit(job.base.main())
