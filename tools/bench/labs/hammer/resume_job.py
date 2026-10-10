"""Charge pressure completion/reconciliation to its existing finite allocation."""
from pathlib import Path
import job

ROOT = Path(__file__).resolve().parents[4]
job.base.PROTOCOL = ROOT / 'docs/experiments/benchmarks/pressure-resumption-protocol.md'

if __name__ == '__main__':
    raise SystemExit(job.base.main())
