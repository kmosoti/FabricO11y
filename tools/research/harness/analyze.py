import research_paths
from pathlib import Path
import json, sys, statistics
def pct(v, p):
    v = sorted(v); k = (len(v) - 1) * p / 100; f = int(k); c = min(f + 1, len(v) - 1)
    return v[f] + (v[c] - v[f]) * (k - f)
STAGES = [("collect_wait", "t0", "t1", "line written → Spindle cycle that read it starts (1 s log poll)"),
          ("spool_commit", "t1", "t2", "read, encode, Spool append with two syncs"),
          ("transit_and_group", "t2", "t3", "node sends, server queues, 50 ms group window closes"),
          ("server_commit_ack", "t3", "t4", "journal two-sync commit, ACK back to the node"),
          ("visible", "t3", "t5", "group commit starts → first query that returns the line (10 ms polling)"),
          ("total", "t0", "t5", "line written → first query that returns it")]
out = {}
for name in sys.argv[1:]:
    d = json.load(open(fstr(research_paths.DATA / "{name}/result.json")))
    rows = [r for r in d["rows"] if not r.get("missing")]
    miss = [r for r in d["rows"] if r.get("missing")]
    res = {"lines": len(d["rows"]), "missing": len(miss), "stages": {}}
    print(f"\n== {name}: {len(rows)} lines seen, {len(miss)} missing; probe Batches in journal {d['probe_records_in_journal']}, acks {d['acks_seen']}")
    print(f"{'stage':20} {'p50':>8} {'p90':>8} {'p99':>8} {'max':>8}  ms")
    for s, a, b, desc in STAGES:
        v = [(r[b] - r[a]) / 1e6 for r in rows if r[a] is not None and r[b] is not None]
        q = {k: round(pct(v, p), 1) for k, p in [("p50", 50), ("p90", 90), ("p99", 99)]}; q["max"] = round(max(v), 1); q["n"] = len(v); q["mean"] = round(statistics.mean(v), 1)
        res["stages"][s] = q
        print(f"{s:20} {q['p50']:8.1f} {q['p90']:8.1f} {q['p99']:8.1f} {q['max']:8.1f}   n={q['n']}")
    rtt = [r["rtt_us"] / 1000 for r in rows if r["rtt_us"]]
    res["node_rtt_ms"] = {"p50": round(pct(rtt, 50), 1), "p99": round(pct(rtt, 99), 1)}
    ql = d["query_ms"]
    res["query_ms"] = {"n": len(ql), "p50": round(pct(ql, 50), 2), "p90": round(pct(ql, 90), 2), "p99": round(pct(ql, 99), 2), "max": round(max(ql), 1)}
    print("node request round trip ms", res["node_rtt_ms"])
    print("query latency ms (keep-alive, logs, ~2 min window)", res["query_ms"])
    out[name] = res
json.dump(out, open(research_paths.DATA / "summary.json", "w"), indent=1)
