"""Hypothesis A4: the walk's no-stop overhead is order, not work. Real-text tail,
stock against the walk in key order (with the frame cache) and in file order."""
import json, os, sys, time
from pathlib import Path
import research_paths
sys.argv = ["x", str(research_paths.DATA / "a4")]
exec(open(Path(__file__).with_name("l21.py")).read().split("states = {")[0] if "if research_paths.SMOKE" not in open(Path(__file__).with_name("l21.py")).read() else open(Path(__file__).with_name("l21.py")).read().split("if research_paths.SMOKE:")[0])
state = tail_state("tail-corpus", Path("/home/user/e2e-runs/l21/gen-tail-corpus/batches.faj"))
p, q, _ = server("stock", state); a, b = window(q)
st, ans = q.post({"kind": "logs", "from_ns": 0, "to_ns": 2**63, "limit": 10000, "page": None}); labels = sorted({row["node"] for row in ans["rows"]}); stop(p)
queries = random_queries(a, b, labels, True, n=100)
res = {}; answers = {}
for label, which, tail, topk, order in [("stock", "stock", "off", "off", ""), ("walk-key", "proto", "index", "on", "key"), ("walk-auto", "proto", "index", "on", "")]:
    os.environ["FABRIC_PROTO_WALK_ORDER"] = order
    p, q, logp = server(which, state, tail, topk); window(q)
    res[label] = {"shapes": bench(q, a, b, True, logp if which != "stock" else None)}
    t = time.perf_counter(); answers[label] = answer_set(q, queries); res[label]["random_set_s"] = round(time.perf_counter() - t, 2); res[label]["mem"] = status(p.pid)
    stop(p)
    for k, v in res[label]["shapes"].items():
        tk = v.get("topk"); print(f"   {label:9} {k:22} {v['p50_ms']:9.2f} ms" + (f" [{tk['processed']}/{tk['items']} order {tk.get('order','-')}]" if tk else ""), flush=True)
    print(f"   {label:9} random {res[label]['random_set_s']} s mem {res[label]['mem']}", flush=True)
for label in ("walk-key", "walk-auto"):
    res[label]["mismatches"] = diff(answers["stock"], answers[label], queries); print(label, "mismatches", len(res[label]["mismatches"]))
json.dump(res, open(ROOT / "a4.json", "w"), indent=1)
for k in kids:
    if k.poll() is None: k.kill()
