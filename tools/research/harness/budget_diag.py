import research_paths
from pathlib import Path
import json, sys, os, time
sys.argv = ["x", str(research_paths.DATA / "budget"), "2-3"]
exec(open(Path(__file__).with_name("budget.py")).read().split("result = {}")[0])
import os as _o; state = ROOT / "states" / _o.environ.get("DIAG_STATE", "seg-steady64")
p, q, logp = server("budget", state, "2000"); a, b = window(q)
body = shapes(a, b)[int(_o.environ.get("DIAG_SHAPE", "3"))][1]
nxt = None
for i in range(int(_o.environ.get("DIAG_PAGES", "8"))):
    mark = os.path.getsize(logp)
    t0 = time.perf_counter(); st, ans = q.post(dict(body, page=nxt)); ms = (time.perf_counter() - t0) * 1000
    tk = [l for l in open(logp, errors="replace").read()[mark:].splitlines() if l.startswith("topk:")]
    bg = budget_of(ans); rows = ans["rows"]
    print(i, f"{ms:7.1f} ms", "rows", len(rows), "first", rows[0].get("time_ns", rows[0].get("observed_ns")) if rows else None, "last", rows[-1].get("time_ns", rows[-1].get("observed_ns")) if rows else None, "budget", bg, "next", bool(ans.get("next_page")), tk[-1] if tk else "", flush=True)
    nxt = ans.get("next_page")
    if not nxt: break
stop(p)
for k in kids:
    if k.poll() is None: k.kill()
