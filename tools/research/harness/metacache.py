"""Bounds page, the empty-window gap: per-query metadata reads. The walk with
FABRIC_PROTO_META_CACHE=on (manifests and row-group bounds kept per process, keyed by
Segment path) against the walk and stock, the fourteen L-21 shapes on three 64-Segment
states, and a random differential with pages against stock.
Usage: metacache.py <out-root>
"""
import json, os, sys, time
from pathlib import Path
import research_paths
exec(open(Path(__file__).with_name("l21.py")).read().split("if research_paths.SMOKE:")[0])

states = {
    "seg-stream64": (True, seg_state("seg-stream64", research_paths.DATA / "stream" / "states" / "seg-stream64")),
    "seg-random64": (True, seg_state("seg-random64", Path(os.environ.get("FABRIC_L25_RANDOM", "/home/user/e2e-runs/corpus-seal/states/seg-corpus64")))),
    "seg-adversarial64": (False, seg_state("seg-adversarial64", Path(os.environ.get("FABRIC_ADVERSARIAL", "/home/user/e2e-runs/topk/states/seg-adversarial64")))),
}
res = {}
for sname, (real, state) in states.items():
    res[sname] = {}
    p, q, _ = server("stock", state); a, b = window(q)
    st, ans = q.post({"kind": "logs", "from_ns": 0, "to_ns": 2**63, "limit": 10000, "page": None}); labels = sorted({row["node"] for row in ans["rows"]}); stop(p)
    queries = random_queries(a, b, labels, real, n=60 if research_paths.SMOKE else 200)
    answers = {}
    for label, which, topk, cache in [("stock", "stock", "off", ""), ("walk", "proto", "on", ""), ("walk+cache", "proto", "on", "on")]:
        os.environ["FABRIC_PROTO_META_CACHE"] = cache
        p, q, logp = server(which, state, "off", topk); window(q)
        rec = {"shapes": bench(q, a, b, real, logp if which != "stock" else None)}
        t = time.perf_counter(); answers[label] = answer_set(q, queries); rec["random_set_s"] = round(time.perf_counter() - t, 2); rec["mem"] = status(p.pid)
        stop(p); res[sname][label] = rec
        for k, v in rec["shapes"].items():
            print(f"   {sname} {label:10} {k:22} {v['p50_ms']:8.2f} ms", flush=True)
        print(f"   {sname} {label:10} random {rec['random_set_s']} s mem {rec['mem']}", flush=True)
    for label in ("walk", "walk+cache"):
        res[sname][label]["mismatches"] = diff(answers["stock"], answers[label], queries); print(sname, label, "mismatches", len(res[sname][label]["mismatches"]), flush=True)
json.dump(res, open(ROOT / "metacache.json", "w"), indent=1)
for k in kids:
    if k.poll() is None: k.kill()
