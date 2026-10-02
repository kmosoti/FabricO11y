"""Ledger L-25: a per-row-group trigram bloom filter against an inverted index.
Two real-text Segment states of equal size, one sealed in stream order and one in
random draw order, with the stock server, the L-04 walk and the walk with the filter
(FABRIC_PROTO_TEXT_FILTER=on). Text shapes of graded selectivity, the cold query that
builds the filters, and a random differential against the stock server.
Usage: l25.py <out-root> (states under research_paths.DATA; the random-draw state
is /home/user/e2e-runs/corpus-seal/states/seg-corpus64 unless FABRIC_L25_RANDOM says otherwise)
"""
import json, os, sys, time
from pathlib import Path
import research_paths
exec(open(Path(__file__).with_name("l21.py")).read().split("if research_paths.SMOKE:")[0])

RANDOM_STATE = Path(os.environ.get("FABRIC_L25_RANDOM", "/home/user/e2e-runs/corpus-seal/states/seg-corpus64"))
states = {
    "seg-stream64": seg_state("seg-stream64", research_paths.DATA / "stream" / "states" / "seg-stream64"),
    "seg-random64": seg_state("seg-random64", RANDOM_STATE),
}
# (name, needle, lines of 16,000 in the corpus holding it)
TOKENS = [("common", "INFO"), ("warn", "WARN"), ("blk", "blk_"), ("jk2", "jk2_init"), ("packet", "PacketResponder"),
          ("ntpd", "ntpd"), ("failed_pw", "Failed password"), ("error", "ERROR"), ("session", "session opened"),
          ("exception", "Exception"), ("none", "zq9"), ("none_long", "no such token anywhere")]
def text_shapes(a, b):
    lo, hi = max(0, a - 60 * S), b + 6 * S
    return [(f"text_{n}", {"kind": "logs", "from_ns": lo, "to_ns": hi, "contains": t, "limit": 100, "page": None}) for n, t in TOKENS]

def bench_text(q, a, b, logp, reps):
    out = {}
    for name, body in text_shapes(a, b):
        ts = []; mark = os.path.getsize(logp) if logp else 0
        for _ in range(reps):
            t = time.perf_counter(); st, ans = q.post(body); ts.append((time.perf_counter() - t) * 1000); assert st == 200, (name, st, ans)
        rec = {"p50_ms": round(pct(ts, 50), 2), "min_ms": round(min(ts), 2), "rows": len(ans.get("rows", [])), "complete": ans.get("complete")}
        if logp:
            lines = open(logp, errors="replace").read()[mark:].splitlines()
            tl = [l for l in lines if l.startswith("topk:")]; fl = [l for l in lines if l.startswith("textfilter:")]
            if tl: rec["topk"] = dict(kv.split("=") for kv in tl[-1].split()[1:])
            if fl: rec["textfilter"] = dict(kv.split("=") for kv in fl[-1].split()[1:])
        out[name] = rec
    return out

reps = 3 if research_paths.SMOKE else 8
res = {}
for sname, state in states.items():
    res[sname] = {}
    p, q, _ = server("stock", state); a, b = window(q)
    st, ans = q.post({"kind": "logs", "from_ns": 0, "to_ns": 2**63, "limit": 10000, "page": None}); labels = sorted({row["node"] for row in ans["rows"]}); stop(p)
    queries = random_queries(a, b, labels, True, n=60 if research_paths.SMOKE else 200)
    for i, body in enumerate(queries):  # make every third logs query a graded text query so the filter is exercised
        if body["kind"] == "logs" and i % 3 == 0: body["contains"] = TOKENS[i % len(TOKENS)][1]
    answers = {}
    for label, which, topk, filt in [("stock", "stock", "off", ""), ("walk", "proto", "on", ""), ("filter", "proto", "on", "on")]:
        os.environ["FABRIC_PROTO_TEXT_FILTER"] = filt
        p, q, logp = server(which, state, "off", topk); window(q)
        rec = {}
        if filt:  # the cold query builds every row group's filter
            mark = os.path.getsize(logp); t = time.perf_counter(); q.post(text_shapes(a, b)[-1][1]); rec["cold_build_ms"] = round((time.perf_counter() - t) * 1000, 2)
            fl = [l for l in open(logp, errors="replace").read()[mark:].splitlines() if l.startswith("textfilter:")]
            if fl: rec["cold"] = dict(kv.split("=") for kv in fl[-1].split()[1:])
        rec["shapes"] = bench_text(q, a, b, logp if which != "stock" else None, reps)
        t = time.perf_counter(); answers[label] = answer_set(q, queries); rec["random_set_s"] = round(time.perf_counter() - t, 2); rec["mem"] = status(p.pid)
        stop(p); res[sname][label] = rec
        for k, v in rec["shapes"].items():
            tf = v.get("textfilter"); tk = v.get("topk")
            print(f"   {sname} {label:7} {k:16} {v['p50_ms']:8.2f} ms rows={v['rows']:3}" + (f" groups {tk['processed']}/{tk['items']}" if tk else "") + (f" skipped={tf['skipped']} kept={tf['kept']}" if tf else ""), flush=True)
        print(f"   {sname} {label:7} random {rec['random_set_s']} s mem {rec['mem']}" + (f" cold {rec.get('cold_build_ms')} ms {rec.get('cold')}" if filt else ""), flush=True)
    for label in ("walk", "filter"):
        res[sname][label]["mismatches"] = diff(answers["stock"], answers[label], queries); print(sname, label, "mismatches", len(res[sname][label]["mismatches"]), flush=True)
    res[sname]["queries"] = len(queries)
json.dump(res, open(ROOT / "l25.json", "w"), indent=1)
for k in kids:
    if k.poll() is None: k.kill()
