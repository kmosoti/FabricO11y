"""Owner-scoped pressure ledger; preserve all historical charges."""
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools/bench/labs/cross_system'))
import run_native_job as base

old_load, old_totals = base.load_ledgers, base.ledger_totals
PREVIOUS = base.BASE
base.BASE = ROOT / 'docs/experiments/benchmarks/data/hammer-reference-01'
base.PROTOCOL = ROOT / 'docs/experiments/benchmarks/hammer-walk-errorbody-protocol.md'
base.CAPS = {'memory': 8192, 'query': 512, 'coordinator': 128}
base.FRONTIER_SECONDS = 36000  # 28,800 historical + 7,200 owner-scoped pressure.


def load():
    ledgers, manifests = old_load()
    paths = sorted((PREVIOUS / 'coordinator').glob('*/receipt.json'))
    if not paths:
        raise RuntimeError('previous native ledger absent')
    ledgers['previous-native-frontier-01'] = [json.loads(p.read_text()) for p in paths]
    manifests['previous-native-frontier-01'] = [dict(path=str(p), sha256=base.sha(p)) for p in paths]
    return ledgers, manifests


def totals(ledgers):
    current = ledgers['native-frontier-01']
    combined = dict(ledgers, **{'native-frontier-01': ledgers['previous-native-frontier-01'] + current})
    values = old_totals(combined)
    previous = sum(r['elapsed_s'] for r in ledgers['previous-native-frontier-01'])
    elapsed = sum(r['elapsed_s'] for r in current)
    values.update(previous_native_s=previous, pressure_round_s=elapsed,
                  remaining_round_s=7200-elapsed, explicit_additional_allocation_s=7200)
    return values


base.load_ledgers, base.ledger_totals = load, totals
if __name__ == '__main__':
    raise SystemExit(base.main())
