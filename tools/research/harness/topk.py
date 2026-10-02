"""Exploratory: the threshold algorithm over tail entries and row groups (ledger L-04).
Usage: topk.py <out-root>
"""
import http.client, json, os, random, shutil, signal, socket, ssl, subprocess, sys, time
from pathlib import Path
import research_paths
from delivery_faults import free_port, make_certs

SB = research_paths.SEALBENCH
BIN = research_paths.BIN
ROOT = Path(sys.argv[1]).resolve(); ROOT.mkdir(parents=True, exist_ok=True)
TAILS = (research_paths.DATA / "tailbench")
GEN = (research_paths.DATA / "retscale/gen-steady")
MIB = 1048576
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

def server(which, state, tail="off", topk="off", disp="off", file_bytes=1024 * MIB):
    port = free_port(); tag = f"{which}-{tail}-{topk}-{disp}"; conf = state / f"{tag}.conf"
    conf.write_text(f"listen=127.0.0.1:{port}\ntls_cert={ROOT}/server.pem\ntls_key={ROOT}/server.key\nstate_dir={state}\nadmin_token_file={ROOT}/admin-token\njournal_bytes=4294967296\njournal_file_bytes={file_bytes}\n")
    env = dict(os.environ, FABRIC_PROTO_TAIL=tail, FABRIC_PROTO_TOPK=topk, FABRIC_PROTO_DISPOSITION=disp)
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

def seg_state(name, files, n):
    state = ROOT / "states" / name; shutil.rmtree(state, ignore_errors=True); (state / "journal").mkdir(parents=True)
    for f in files[:n]: os.link(f, state / "journal" / f.name)
    p, q, _ = server("stock", state, file_bytes=65536)
    t0 = time.monotonic()
    while list((state / "journal").glob("sealed-*")) or len(list((state / "segments").glob("seg-*"))) < n:
        if time.monotonic() - t0 > 600: raise SystemExit("sealing stalled")
        time.sleep(0.2)
    stop(p); return state

def window(q):
    st, ans = q.post({"kind": "logs", "from_ns": 0, "to_ns": 2**63, "limit": 1, "page": None}); return ans["retained_from_ns"], ans["retained_to_ns"]

def shapes(a, b):
    s = 1_000_000_000; lo, hi = max(0, a - 60 * s), b + 6 * s
    return [
        ("logs_limit50_full",     {"kind": "logs", "from_ns": lo, "to_ns": hi, "limit": 50, "page": None}),
        ("logs_limit1000_full",   {"kind": "logs", "from_ns": lo, "to_ns": hi, "limit": 1000, "page": None}),
        ("logs_limit10000_full",  {"kind": "logs", "from_ns": lo, "to_ns": hi, "limit": 10000, "page": None}),
        ("host_limit50_full",     {"kind": "logs", "node": "node-0007", "from_ns": lo, "to_ns": hi, "limit": 50, "page": None}),
        ("metric_limit100_full",  {"kind": "metrics", "name": "sim.metric.3", "from_ns": lo, "to_ns": hi, "limit": 100, "page": None}),
        ("metric_limit10000_full",{"kind": "metrics", "name": "sim.metric.3", "from_ns": lo, "to_ns": hi, "limit": 10000, "page": None}),
        ("text_rare_full",        {"kind": "logs", "from_ns": lo, "to_ns": hi, "contains": "zq9", "limit": 100, "page": None}),
        ("rate_full",             {"kind": "rate", "name": "sim.metric.0", "from_ns": lo, "to_ns": hi}),
        ("logs_last_10s",         {"kind": "logs", "from_ns": b - 10 * s, "to_ns": hi, "limit": 100, "page": None}),
        ("empty_past",            {"kind": "logs", "from_ns": 1, "to_ns": 2, "limit": 100, "page": None}),
    ]

def bench(q, a, b, logp=None, reps=8):
    out = {}
    for name, body in shapes(a, b):
        ts = []; mark = os.path.getsize(logp) if logp else 0
        for _ in range(reps):
            t = time.perf_counter(); st, ans = q.post(body); ts.append((time.perf_counter() - t) * 1000); assert st == 200, (name, st, ans)
        rec = {"p50_ms": round(pct(ts, 50), 2), "min_ms": round(min(ts), 2), "rows": len(ans.get("rows", [])), "complete": ans.get("complete")}
        if logp:
            tail_lines = [l for l in open(logp, errors="replace").read()[mark:].splitlines() if l.startswith("topk:")]
            if tail_lines: rec["topk"] = dict(kv.split("=") for kv in tail_lines[-1].split()[1:])
        # page 2 of the limit-50 shape exercises the lower bound
        if name == "logs_limit50_full" and ans.get("next_page"):
            ts2 = []
            for _ in range(reps):
                t = time.perf_counter(); st2, ans2 = q.post(dict(body, page=ans["next_page"])); ts2.append((time.perf_counter() - t) * 1000)
            rec["page2_p50_ms"] = round(pct(ts2, 50), 2)
        out[name] = rec
    return out

def random_queries(a, b, labels, n=300, seed=23):
    rng = random.Random(seed); s = 1_000_000_000; out = []
    for _ in range(n):
        frm = rng.randint(a - 10 * s, b + 5 * s); to = frm + int(rng.choice([rng.uniform(0.5, 30), rng.uniform(30, 600), 10**6]) * s)
        node = rng.choice([None, None, None, rng.choice(labels), "node-9999"])
        kind = rng.choice(["logs", "logs", "metrics", "rate"]); body = {"kind": kind, "from_ns": frm, "to_ns": to}
        if node: body["node"] = node
        if kind == "logs":
            body.update({"limit": rng.choice([1, 10, 50, 1000, 10000]), "page": None})
            if rng.random() < 0.3: body["contains"] = rng.choice(["R", "zq9", "gap"])
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

result = {}
# states
states = {}
states["tail-steady"] = ("tail", tail_state("tail-steady", TAILS / "tail-steady" / "batches.faj"))
states["tail-outage"] = ("tail", tail_state("tail-outage", TAILS / "tail-outage" / "batches.faj"))
steady_files = sorted(GEN.glob("sealed-*.faj"))
states["seg-steady64"] = ("seg", seg_state("seg-steady64", steady_files, 64))
adv = ROOT / "gen-adversarial"
if not list(adv.glob("sealed-*.faj")):
    subprocess.run([str(SB), "gen", "adversarial", str(adv), str(64 * MIB), str(MIB)], check=True, capture_output=True, timeout=1800)
states["seg-adversarial64"] = ("seg", seg_state("seg-adversarial64", sorted(adv.glob("sealed-*.faj")), 64))

MODES = {"tail": [("stock", "off", "off"), ("index", "index", "off"), ("index+topk", "index", "on")],
         "seg": [("stock", "off", "off"), ("topk", "off", "on")]}
try:
    for name, (kindof, state) in states.items():
        print(f"== {name}", flush=True); r = {}
        p, q, _ = server("stock", state); a, b = window(q)
        st, ans = q.post({"kind": "logs", "from_ns": 0, "to_ns": 2**63, "limit": 10000, "page": None}); labels = sorted({row["node"] for row in ans["rows"]}); stop(p)
        queries = random_queries(a, b, labels); answers = {}
        for label, tail, topk in MODES[kindof]:
            which = "stock" if label == "stock" else "proto"
            p, q, logp = server(which, state, tail, topk)
            window(q); m0 = status(p.pid)
            r[label] = {"shapes": bench(q, a, b, logp if which == "proto" else None), "mem_after_shapes_MiB": status(p.pid)}
            t = time.perf_counter(); answers[label] = answer_set(q, queries); r[label]["random_set_s"] = round(time.perf_counter() - t, 2); r[label]["mem_after_random_MiB"] = status(p.pid)
            stop(p)
            for k, v in r[label]["shapes"].items():
                extra = f" page2 {v['page2_p50_ms']:.1f}" if "page2_p50_ms" in v else ""
                tk = v.get("topk"); tks = f" [items {tk['items']} processed {tk['processed']} stopped {tk['stopped']}]" if tk else ""
                print(f"   {label:11} {k:24} {v['p50_ms']:9.2f} ms rows {v['rows']:5}{extra}{tks}", flush=True)
            print(f"   {label:11} random set {r[label]['random_set_s']} s  mem {r[label]['mem_after_random_MiB']}", flush=True)
        best = MODES[kindof][-1][0]
        r["differential"] = {"queries": len(queries), "mismatches": diff(answers["stock"], answers[best], queries), "compared": best}
        print(f"   differential stock vs {best}: {len(r['differential']['mismatches'])} mismatches", flush=True)
        result[name] = r; json.dump(result, open(ROOT / "topk.json", "w"), indent=1)
finally:
    for p in kids:
        if p.poll() is None: p.kill(); p.wait(timeout=10)
