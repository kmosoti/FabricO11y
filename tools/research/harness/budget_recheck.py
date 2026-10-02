import research_paths
from pathlib import Path
"""Re-check the random queries the budget harness flagged, draining both servers to the end."""
import json, sys, time
sys.argv = ["x", str(research_paths.DATA / "budget"), "0-1"]
src = open(Path(__file__).with_name("budget.py")).read().split("result = {}")[0]
exec(src)
flagged = json.load(open(ROOT / "budget.json"))
out = {}
for state_name, modes in flagged.items():
    for mode, r in modes.items():
        if not mode.startswith("budget") or not r.get("random_mismatches"): continue
        state = ROOT / "states" / state_name; budget = mode.split("-")[1]
        # one server at a time on a state: the journal's writer lock is exclusive
        ps, qs, _ = server("stock", state); window(qs)
        stock_drains = [drain(qs, m["query"], cap=5000) for m in r["random_mismatches"]]
        stop(ps)
        pb, qb, _ = server("budget", state, budget); window(qb)
        for m, a in zip(r["random_mismatches"], stock_drains):
            body = m["query"]
            b = drain(qb, body, cap=5000)
            ra = [row for st, ans, _ in a if st == 200 for row in ans["rows"]]
            rb = [row for st, ans, _ in b if st == 200 for row in ans["rows"]]
            iss = check_pages(b, ra, body["kind"])
            rec = {"state": state_name, "mode": mode, "index": m["index"], "stock_pages": len(a), "budget_pages": len(b), "stock_rows": len(ra), "budget_rows": len(rb), "equal": ra == rb, "issues": iss, "capped": len(a) >= 5000 or len(b) >= 5000}
            print(rec, flush=True); out[f"{state_name}/{mode}/{m['index']}"] = rec
        stop(pb)
json.dump(out, open(ROOT / "recheck.json", "w"), indent=1)
for p in kids:
    if p.poll() is None: p.kill()
