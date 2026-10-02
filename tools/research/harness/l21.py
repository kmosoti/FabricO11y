"""Exploratory: the real-corpus query run (ledger L-21). Same shapes on a real-text and a
synthetic tail and Segment state, stock server and the L-04 walk prototype.
Usage: l21.py <out-root>
"""
import http.client, json, os, random, shutil, signal, socket, ssl, subprocess, sys, time
from pathlib import Path
import research_paths
from delivery_faults import free_port, make_certs

BIN = research_paths.BIN
ROOT = Path(sys.argv[1]).resolve(); ROOT.mkdir(parents=True, exist_ok=True)
MIB = 1048576; S = 1_000_000_000
make_certs(ROOT); TOKEN = "ab" * 32; (ROOT / "admin-token").write_text(TOKEN + "\n")
kids = []

def pct(v, p):
    v = sorted(v); k = (len(v) - 1) * p / 100; f = int(k); c = min(f + 1, len(v) - 1)
    return v[f] + (v[c] - v[f]) * (k - f)
def status(pid):
    d = {}
    for l in open(f"/proc/{pid}/status"):
        if l.startswith(("VmHWM", "VmRSS")): d[l.split(":")[0]] = int(l.split()[1]) // 1024
    return d

class Q:
    def __init__(self, port):
        self.port = port; self.ctx = ssl.create_default_context(cafile=str(ROOT / "ca.pem")); self.conn = None
    def post(self, body):
        if self.conn is None: self.conn = http.client.HTTPSConnection("127.0.0.1", self.port, context=self.ctx, timeout=600)
        self.conn.request("POST", "/v1/admin/query", body=json.dumps(body).encode(), headers={"authorization": f"Bearer {TOKEN}", "content-type": "application/json"})
        r = self.conn.getresponse(); return r.status, json.loads(r.read())

def server(which, state, tail="off", topk="off"):
    port = free_port(); tag = f"{which}-{tail}-{topk}"; conf = state / f"{tag}.conf"
    conf.write_text(f"listen=127.0.0.1:{port}\ntls_cert={ROOT}/server.pem\ntls_key={ROOT}/server.key\nstate_dir={state}\nadmin_token_file={ROOT}/admin-token\njournal_bytes=4294967296\njournal_file_bytes={1024 * MIB}\n")
    env = dict(os.environ, FABRIC_PROTO_TAIL=tail, FABRIC_PROTO_TOPK=topk, FABRIC_PROTO_DISPOSITION="off"); env.pop("FABRIC_PROTO_BUDGET", None)
    logp = state / f"{tag}.log"; log = open(logp, "wb"); t0 = time.monotonic()
    p = subprocess.Popen(["taskset", "-c", "0-1", str(BIN / which / "fabric-server"), "serve", str(conf)], stdout=log, stderr=subprocess.STDOUT, env=env); kids.append(p)
    while True:
        try: socket.create_connection(("127.0.0.1", port), timeout=0.05).close(); break
        except OSError:
            if p.poll() is not None or time.monotonic() - t0 > 900: raise SystemExit(f"{which} did not start on {state}")
            time.sleep(0.02)
    return p, Q(port), logp
def stop(p): p.send_signal(signal.SIGTERM); p.wait(timeout=600)

def tail_state(name, src):
    state = ROOT / "states" / name; shutil.rmtree(state, ignore_errors=True); (state / "journal").mkdir(parents=True)
    os.link(src, state / "journal" / "batches.faj"); return state
def seg_state(name, src):
    state = ROOT / "states" / name; shutil.rmtree(state, ignore_errors=True)
    subprocess.run(["cp", "-al", str(src), str(state)], check=True)
    for f in list(state.glob("*.conf")) + list(state.glob("*.log")): f.unlink()
    return state

def window(q):
    st, ans = q.post({"kind": "logs", "from_ns": 0, "to_ns": 2**63, "limit": 1, "page": None}); return ans["retained_from_ns"], ans["retained_to_ns"]

def shapes(a, b, real):
    lo, hi = max(0, a - 60 * S), b + 6 * S
    common, mid, rare, none = ("INFO", "ERROR", "Exception", "zq9") if real else ("RRRR", "/fUL", "RklbD", "zq9")
    return [
        ("logs_limit50_full",     {"kind": "logs", "from_ns": lo, "to_ns": hi, "limit": 50, "page": None}),
        ("logs_limit1000_full",   {"kind": "logs", "from_ns": lo, "to_ns": hi, "limit": 1000, "page": None}),
        ("logs_limit10000_full",  {"kind": "logs", "from_ns": lo, "to_ns": hi, "limit": 10000, "page": None}),
        ("host_limit50_full",     {"kind": "logs", "node": "node-0007", "from_ns": lo, "to_ns": hi, "limit": 50, "page": None}),
        ("metric_limit100_full",  {"kind": "metrics", "name": "sim.metric.3", "from_ns": lo, "to_ns": hi, "limit": 100, "page": None}),
        ("text_common_full",      {"kind": "logs", "from_ns": lo, "to_ns": hi, "contains": common, "limit": 100, "page": None}),
        ("text_mid_full",         {"kind": "logs", "from_ns": lo, "to_ns": hi, "contains": mid, "limit": 100, "page": None}),
        ("text_rare_full",        {"kind": "logs", "from_ns": lo, "to_ns": hi, "contains": rare, "limit": 100, "page": None}),
        ("text_none_full",        {"kind": "logs", "from_ns": lo, "to_ns": hi, "contains": none, "limit": 100, "page": None}),
        ("text_none_60s",         {"kind": "logs", "from_ns": b - 60 * S, "to_ns": hi, "contains": none, "limit": 100, "page": None}),
        ("rate_full",             {"kind": "rate", "name": "sim.metric.0", "from_ns": lo, "to_ns": hi}),
        ("logs_last_10s",         {"kind": "logs", "from_ns": b - 10 * S, "to_ns": hi, "limit": 100, "page": None}),
        ("host_last_60s",         {"kind": "logs", "node": "node-0007", "from_ns": b - 60 * S, "to_ns": hi, "limit": 100, "page": None}),
        ("empty_past",            {"kind": "logs", "from_ns": 1, "to_ns": 2, "limit": 100, "page": None}),
    ]

def bench(q, a, b, real, logp=None, reps=3 if research_paths.SMOKE else 8):
    out = {}
    for name, body in shapes(a, b, real):
        ts = []; mark = os.path.getsize(logp) if logp else 0
        for _ in range(reps):
            t = time.perf_counter(); st, ans = q.post(body); ts.append((time.perf_counter() - t) * 1000); assert st == 200, (name, st, ans)
        rec = {"p50_ms": round(pct(ts, 50), 2), "min_ms": round(min(ts), 2), "rows": len(ans.get("rows", [])), "complete": ans.get("complete"), "body_bytes": sum(len(r.get("body", "")) for r in ans.get("rows", []))}
        if logp:
            tl = [l for l in open(logp, errors="replace").read()[mark:].splitlines() if l.startswith("topk:")]
            if tl: rec["topk"] = dict(kv.split("=") for kv in tl[-1].split()[1:])
        out[name] = rec
    return out

def random_queries(a, b, labels, real, n=60 if research_paths.SMOKE else 200, seed=41):
    rng = random.Random(seed); out = []
    toks = ["INFO", "ERROR", "Exception", "blk_", "zq9"] if real else ["R", "zq9", "gap", "/fUL"]
    for _ in range(n):
        frm = rng.randint(a - 10 * S, b + 5 * S); to = frm + int(rng.choice([rng.uniform(0.5, 30), rng.uniform(30, 600), 10**6]) * S)
        node = rng.choice([None, None, None, rng.choice(labels), "node-9999"])
        kind = rng.choice(["logs", "logs", "metrics", "rate"]); body = {"kind": kind, "from_ns": frm, "to_ns": to}
        if node: body["node"] = node
        if kind == "logs":
            body.update({"limit": rng.choice([1, 10, 50, 1000, 10000]), "page": None})
            if rng.random() < 0.4: body["contains"] = rng.choice(toks)
        elif kind == "metrics": body.update({"name": f"sim.metric.{rng.randrange(32)}", "limit": rng.choice([1, 10, 100, 10000]), "page": None})
        else: body["name"] = f"sim.metric.{rng.randrange(32)}"
        out.append(body)
    return out

def answer_set(q, queries):
    got = []
    for body in queries:
        st, ans = q.post(body); entry = {"first": [st, ans], "pages": []}
        nxt = ans.get("next_page") if st == 200 else None; hops = 0
        while nxt and hops < 3:
            st2, ans2 = q.post(dict(body, page=nxt)); entry["pages"].append([st2, ans2]); nxt = ans2.get("next_page") if st2 == 200 else None; hops += 1
        got.append(entry)
    return got

def diff(a, b, queries):
    bad = []
    for i, (x, y) in enumerate(zip(a, b)):
        if x != y:
            fx, fy = x["first"][1], y["first"][1]
            fields = sorted(k for k in set(fx) | set(fy) if fx.get(k) != fy.get(k)) if x["first"][0] == y["first"][0] == 200 else ["status"]
            bad.append({"index": i, "query": queries[i], "fields": fields, "pages_equal": x["pages"] == y["pages"]})
    return bad

if research_paths.SMOKE:
    gen = ROOT / "gen-tail-smoke"
    if not (gen / "batches.faj").exists():
        env = dict(os.environ, SEALBENCH_CORPUS=str(research_paths.CORPUS / "all.log"))
        subprocess.run([str(research_paths.SEALBENCH), "gen", "steady", str(gen), str(16 * MIB), str(2048 * MIB)], check=True, env=env, timeout=1800)
    states = {"tail-corpus-16": ("tail", True, tail_state("tail-corpus-16", gen / "batches.faj"))}
else:
    states = {
        "tail-corpus": ("tail", True, tail_state("tail-corpus", research_paths.DATA / "l21" / "gen-tail-corpus" / "batches.faj")),
        "tail-steady": ("tail", False, tail_state("tail-steady", research_paths.DATA / "tailbench" / "tail-steady" / "batches.faj")),
        "seg-corpus64": ("seg", True, seg_state("seg-corpus64", research_paths.DATA / "corpus-seal" / "states" / "seg-corpus64")),
        "seg-steady64": ("seg", False, seg_state("seg-steady64", research_paths.DATA / "topk" / "states" / "seg-steady64")),
    }
MODES = {"tail": [("stock", "stock", "off", "off"), ("walk", "budget", "index", "on")], "seg": [("stock", "stock", "off", "off"), ("walk", "budget", "off", "on")]}
result = {}
try:
    for name, (kindof, real, state) in states.items():
        print(f"== {name}", flush=True); r = {}
        p, q, _ = server("stock", state); a, b = window(q); t0 = time.perf_counter()
        st, ans = q.post({"kind": "logs", "from_ns": 0, "to_ns": 2**63, "limit": 10000, "page": None}); labels = sorted({row["node"] for row in ans["rows"]}); stop(p)
        queries = random_queries(a, b, labels, real); answers = {}
        for label, which, tail, topk in MODES[kindof]:
            p, q, logp = server(which, state, tail, topk)
            t = time.perf_counter(); window(q); first_ms = (time.perf_counter() - t) * 1000
            r[label] = {"first_query_ms": round(first_ms, 1), "shapes": bench(q, a, b, real, logp if which != "stock" else None), "mem_after_shapes_MiB": status(p.pid)}
            t = time.perf_counter(); answers[label] = answer_set(q, queries); r[label]["random_set_s"] = round(time.perf_counter() - t, 2); r[label]["mem_after_random_MiB"] = status(p.pid)
            stop(p)
            for k, v in r[label]["shapes"].items():
                tk = v.get("topk"); tks = f" [{tk['processed']}/{tk['items']} stop {tk['stopped']}]" if tk else ""
                print(f"   {label:6} {k:22} {v['p50_ms']:9.2f} ms rows {v['rows']:5} body {v['body_bytes']:7}{tks}", flush=True)
            print(f"   {label:6} first {r[label]['first_query_ms']} ms  random set {r[label]['random_set_s']} s  mem {r[label]['mem_after_random_MiB']}", flush=True)
        r["differential"] = {"queries": len(queries), "mismatches": diff(answers["stock"], answers["walk"], queries)}
        print(f"   differential stock vs walk: {len(r['differential']['mismatches'])} mismatches", flush=True)
        result[name] = r; json.dump(result, open(ROOT / "l21.json", "w"), indent=1)
finally:
    for p in kids:
        if p.poll() is None: p.kill(); p.wait(timeout=10)
