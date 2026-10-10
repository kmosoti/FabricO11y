"""Fresh source/binary binding after the journal-identity repair."""
import os
from pathlib import Path
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0,str(ROOT/'tools/bench/labs/readiness_service'))
import research

research.require_limits()
out = Path(sys.argv[1])
out.mkdir(parents=True,exist_ok=False)
research.PROTOCOL = ROOT/'docs/experiments/benchmarks/hammer-journal-identity-protocol.md'
research.build(out,time.monotonic()+560)
(out/'build-wrapper.py').write_bytes(Path(__file__).read_bytes())
print(out/'build.json')
