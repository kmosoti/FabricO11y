"""Hypothesis A3 through the server: the real-text 64 MiB tail answered from FOB1 blocks
(Zstd-compressed blocks of n consecutive journal frames, FABRIC_PROTO_TAIL_BLOCKS=on and
FABRIC_PROTO_TAIL_BLOCK_FRAMES=n swept over FABRIC_A3_SWEEP (default 1,8,32,128,512), built on the
first query as a server writing blocks on receipt would have) against the walk over the
OTLP tail and against stock. The fourteen L-21 shapes and a 200-query random differential
with pages against stock.
Usage: a3.py <out-root>
"""
import json, os, sys, time
from pathlib import Path
import research_paths
exec(open(Path(__file__).with_name("l21.py")).read().split("if research_paths.SMOKE:")[0])

src = Path(os.environ.get("FABRIC_A3_TAIL", "/home/user/e2e-runs/l21/gen-tail-corpus/batches.faj"))
state = tail_state("tail-corpus", src)
p, q, _ = server("stock", state); a, b = window(q)
st, ans = q.post({"kind": "logs", "from_ns": 0, "to_ns": 2**63, "limit": 10000, "page": None}); labels = sorted({row["node"] for row in ans["rows"]}); stop(p)
queries = random_queries(a, b, labels, True, n=60 if research_paths.SMOKE else 200)
res = {}; answers = {}
SWEEP = [int(n) for n in os.environ.get("FABRIC_A3_SWEEP", "1,8,32,128,512").split(",")]
FILTER = os.environ.get("FABRIC_A3_FILTER") == "1"  # also each block size with L-25's trigram bloom per block
MODES = [("stock", "stock", "off", "off", "", 1, ""), ("walk", "proto", "index", "on", "", 1, "")] + [(f"blocks-{n}", "proto", "index", "on", "on", n, "") for n in SWEEP] + ([(f"blocks-{n}+filter", "proto", "index", "on", "on", n, "on") for n in SWEEP] if FILTER else [])
for label, which, tail, topk, blocks, per, filt in MODES:
    os.environ["FABRIC_PROTO_TAIL_BLOCKS"] = blocks; os.environ["FABRIC_PROTO_TAIL_BLOCK_FRAMES"] = str(per); os.environ["FABRIC_PROTO_TEXT_FILTER"] = filt
    p, q, logp = server(which, state, tail, topk); window(q)
    rec = {}
    if which != "stock":  # the cold query: builds the tail index, and with blocks every frame's block
        mark = os.path.getsize(logp); t = time.perf_counter()
        q.post({"kind": "logs", "from_ns": 0, "to_ns": 2**63, "contains": "zq9", "limit": 100, "page": None}); rec["cold_ms"] = round((time.perf_counter() - t) * 1000, 1)
        rec["cold_log"] = [l for l in open(logp, errors="replace").read()[mark:].splitlines() if l.startswith(("tailblocks:", "topk:"))]
    rec["shapes"] = bench(q, a, b, True, logp if which != "stock" else None)
    t = time.perf_counter(); answers[label] = answer_set(q, queries); rec["random_set_s"] = round(time.perf_counter() - t, 2); rec["mem"] = status(p.pid)
    stop(p); res[label] = rec
    for k, v in rec["shapes"].items():
        tk = v.get("topk"); print(f"   {label:7} {k:22} {v['p50_ms']:9.2f} ms" + (f" [{tk['processed']}/{tk['items']}]" if tk else ""), flush=True)
    print(f"   {label:7} random {rec['random_set_s']} s mem {rec['mem']} cold {rec.get('cold_ms')} {rec.get('cold_log')}", flush=True)
for label in [m[0] for m in MODES[1:]]:
    res[label]["mismatches"] = diff(answers["stock"], answers[label], queries); print(label, "mismatches", len(res[label]["mismatches"]), flush=True)
json.dump(res, open(ROOT / "a3.json", "w"), indent=1)
for k in kids:
    if k.poll() is None: k.kill()
