"""ADR-0024 part 2 in the product: Segments sealed by the product server carry a
trigram filter per logs row group. query_plan=scan against query_plan=walk on 64
real-text Segments in stream order and 64 in random-draw order: twelve tokens of graded
frequency, the L-21 shapes, a 200-query differential with pages (a third of the logs
queries carrying a token), and the filters' bytes against the logs table's.
Usage: product_filter.py <out-root> <stream-state> <random-state>
"""
import json, os, sys, time
from pathlib import Path
import research_paths
argv = sys.argv[:]; sys.argv = argv[:2]
exec(open(Path(__file__).with_name("l21.py")).read().split("if research_paths.SMOKE:")[0])
states = {"seg-stream64f": seg_state("seg-stream64f", Path(argv[2])), "seg-random64f": seg_state("seg-random64f", Path(argv[3]))}
TOKENS = [("common", "INFO"), ("warn", "WARN"), ("blk", "blk_"), ("jk2", "jk2_init"), ("packet", "PacketResponder"),
          ("ntpd", "ntpd"), ("failed_pw", "Failed password"), ("error", "ERROR"), ("session", "session opened"),
          ("exception", "Exception"), ("none", "zq9"), ("none_long", "no such token anywhere")]
def token_bench(q, a, b, reps):
    lo, hi = max(0, a - 60 * S), b + 6 * S; out = {}
    for n, t in TOKENS:
        body = {"kind": "logs", "from_ns": lo, "to_ns": hi, "contains": t, "limit": 100, "page": None}; ts = []
        for _ in range(reps):
            t0 = time.perf_counter(); st, ans = q.post(body); ts.append((time.perf_counter() - t0) * 1000); assert st == 200
        out[f"text_{n}"] = {"p50_ms": round(pct(ts, 50), 2), "rows": len(ans["rows"])}
    return out
res = {}
for sname, state in states.items():
    segs = list((state / "segments").glob("seg-*"))
    res[sname] = {"filter_bytes": sum((d / "text_filter.bin").stat().st_size for d in segs), "logs_bytes": sum((d / "logs.parquet").stat().st_size for d in segs)}
    os.environ["FABRIC_QUERY_PLAN"] = "scan"
    p, q, _ = server("product", state); a, b = window(q)
    st, ans = q.post({"kind": "logs", "from_ns": 0, "to_ns": 2**63, "limit": 10000, "page": None}); labels = sorted({row["node"] for row in ans["rows"]}); stop(p)
    queries = random_queries(a, b, labels, True, n=200)
    for i, body in enumerate(queries):
        if body["kind"] == "logs" and i % 3 == 0: body["contains"] = TOKENS[i % len(TOKENS)][1]
    answers = {}
    for plan in ("scan", "walk"):
        os.environ["FABRIC_QUERY_PLAN"] = plan
        p, q, _ = server("product", state); window(q)
        t0 = time.perf_counter(); q.post({"kind": "logs", "from_ns": 0, "to_ns": 2**63, "contains": "zq9", "limit": 100, "page": None}); cold = round((time.perf_counter() - t0) * 1000, 1)
        rec = {"cold_ms": cold, "tokens": token_bench(q, a, b, 8), "shapes": bench(q, a, b, True)}
        t0 = time.perf_counter(); answers[plan] = answer_set(q, queries); rec["random_set_s"] = round(time.perf_counter() - t0, 2); rec["mem"] = status(p.pid)
        stop(p); res[sname][plan] = rec
        for k, v in rec["tokens"].items(): print(f"   {sname} {plan} {k:16} {v['p50_ms']:8.2f} ms rows={v['rows']}", flush=True)
        print(f"   {sname} {plan} random {rec['random_set_s']} s mem {rec['mem']} cold {cold}", flush=True)
    res[sname]["mismatches"] = diff(answers["scan"], answers["walk"], queries); print(sname, "mismatches", len(res[sname]["mismatches"]), "filter bytes", res[sname]["filter_bytes"], "logs bytes", res[sname]["logs_bytes"], flush=True)
json.dump(res, open(ROOT / "product_filter.json", "w"), indent=1)
for k in kids:
    if k.poll() is None: k.kill()
