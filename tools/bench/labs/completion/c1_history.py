"""Execute the deferred C1-6 unchanged in a fresh campaign destination."""
from pathlib import Path
import sys
ROOT=Path(__file__).resolve().parents[4]
sys.path.insert(0,str(ROOT/'tools/bench/labs/dev_small'))
import run
run.DATA=ROOT/'docs/experiments/benchmarks/data/lab-completion-run-01'
sys.argv=[__file__,'--cell','c1-6']
run.main()
