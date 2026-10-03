"""ADR-0024 part 1 in the product: one release binary of the product tree with
query_plan=scan against query_plan=walk, the fourteen L-21 shapes on the real-text
64 MiB tail and on 64 stream-ordered real-text Segments, and a 200-query random
differential with pages between the two plans.
Usage: product_walk.py <out-root>   (binary at research_paths.BIN / "product")
"""
import json, os, sys, time
from pathlib import Path
import research_paths
exec(open(Path(__file__).with_name("l21.py")).read().split("if research_paths.SMOKE:")[0])

states = {
    "tail-corpus": tail_state("tail-corpus", Path(os.environ.get("FABRIC_A3_TAIL", "/home/user/e2e-runs/l21/gen-tail-corpus/batches.faj"))),
    "seg-stream64": seg_state("seg-stream64", research_paths.DATA / "stream" / "states" / "seg-stream64"),
}
res = {}
for sname, state in states.items():
    os.environ["FABRIC_QUERY_PLAN"] = "scan"
    p, q, _ = server("product", state); a, b = window(q)
    st, ans = q.post({"kind": "logs", "from_ns": 0, "to_ns": 2**63, "limit": 10000, "page": None}); labels = sorted({row["node"] for row in ans["rows"]}); stop(p)
    queries = random_queries(a, b, labels, True, n=60 if research_paths.SMOKE else 200)
    res[sname] = {}; answers = {}
    for plan in ("scan", "walk"):
        os.environ["FABRIC_QUERY_PLAN"] = plan
        p, q, _ = server("product", state); window(q)
        t = time.perf_counter(); q.post({"kind": "logs", "from_ns": 0, "to_ns": 2**63, "contains": "zq9", "limit": 100, "page": None}); cold = round((time.perf_counter() - t) * 1000, 1)
        rec = {"cold_ms": cold, "shapes": bench(q, a, b, True)}
        t = time.perf_counter(); answers[plan] = answer_set(q, queries); rec["random_set_s"] = round(time.perf_counter() - t, 2); rec["mem"] = status(p.pid)
        stop(p); res[sname][plan] = rec
        for k, v in rec["shapes"].items(): print(f"   {sname} {plan} {k:22} {v['p50_ms']:9.2f} ms", flush=True)
        print(f"   {sname} {plan} random {rec['random_set_s']} s mem {rec['mem']} cold {cold}", flush=True)
    res[sname]["mismatches"] = diff(answers["scan"], answers["walk"], queries); print(sname, "mismatches", len(res[sname]["mismatches"]), flush=True)
json.dump(res, open(ROOT / "product_walk.json", "w"), indent=1)
for k in kids:
    if k.poll() is None: k.kill()
