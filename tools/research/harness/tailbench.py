"""Exploratory: a key index of the unsealed journal tail (ledger L-03).
Usage: tailbench.py <out-root> [tails,comma,separated]
"""
import http.client, json, os, random, shutil, signal, socket, ssl, subprocess, sys, threading, time
from pathlib import Path
import research_paths
from delivery_faults import free_port, make_certs

SB = research_paths.SEALBENCH
BIN = research_paths.BIN
ROOT = Path(sys.argv[1]).resolve()
TAILS = sys.argv[2].split(",") if len(sys.argv) > 2 else ["steady", "tiny", "needle", "outage", "skew"]
MIB = 1048576
ROOT.mkdir(parents=True, exist_ok=True)
make_certs(ROOT)
TOKEN = "ef" * 32
(ROOT / "admin-token").write_text(TOKEN + "\n")
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
        if self.conn is None:
            self.conn = http.client.HTTPSConnection("127.0.0.1", self.port, context=self.ctx, timeout=600)
        self.conn.request("POST", "/v1/admin/query", body=json.dumps(body).encode(),
                          headers={"authorization": f"Bearer {TOKEN}", "content-type": "application/json"})
        r = self.conn.getresponse(); data = r.read()
        return r.status, json.loads(data)

def gen_tail(kind):
    d = ROOT / f"tail-{kind}"
    if not (d / "batches.faj").exists() or (d / "batches.faj").stat().st_size < 60 * MIB:
        shutil.rmtree(d, ignore_errors=True)
        subprocess.run([str(SB), "gen", kind, str(d), str(64 * MIB), str(2048 * MIB)], check=True, capture_output=True, timeout=3600)
    return d / "batches.faj"

def make_state(name, tail):
    state = ROOT / "states" / name
    shutil.rmtree(state, ignore_errors=True); (state / "journal").mkdir(parents=True)
    os.link(tail, state / "journal" / "batches.faj")
    return state

def server(which, state, tail_mode="off", disp="off", file_bytes=1024 * MIB):
    port = free_port(); conf = state / f"{which}-{tail_mode}-{disp}.conf"
    conf.write_text(f"listen=127.0.0.1:{port}\ntls_cert={ROOT}/server.pem\ntls_key={ROOT}/server.key\n"
                    f"state_dir={state}\nadmin_token_file={ROOT}/admin-token\njournal_bytes=4294967296\njournal_file_bytes={file_bytes}\n")
    env = dict(os.environ, FABRIC_PROTO_TAIL=tail_mode, FABRIC_PROTO_DISPOSITION=disp)
    logp = state / f"{which}-{tail_mode}-{disp}.log"; log = open(logp, "wb")
    t0 = time.monotonic()
    p = subprocess.Popen(["taskset", "-c", "0-1", str(BIN / which / "fabric-server"), "serve", str(conf)], stdout=log, stderr=subprocess.STDOUT, env=env)
    kids.append(p)
    while True:
        try: socket.create_connection(("127.0.0.1", port), timeout=0.05).close(); break
        except OSError:
            if p.poll() is not None or time.monotonic() - t0 > 900: raise SystemExit(f"{which} did not start on {state}")
            time.sleep(0.02)
    return p, Q(port), round(time.monotonic() - t0, 2), logp

def stop(p):
    p.send_signal(signal.SIGTERM); p.wait(timeout=600)

def index_lines(logp):
    out = []
    for l in open(logp, errors="replace"):
        if l.startswith("tail-index:"):
            out.append(dict(kv.split("=") for kv in l.split()[1:]))
    return out

def window(q):
    st, ans = q.post({"kind": "logs", "from_ns": 0, "to_ns": 2**63, "limit": 1, "page": None})
    return ans["retained_from_ns"], ans["retained_to_ns"]

def shapes(a, b, node):
    s = 1_000_000_000
    return [
        ("empty_past",        {"kind": "logs", "from_ns": 1, "to_ns": 2, "limit": 100, "page": None}),
        ("logs_last_10s",     {"kind": "logs", "from_ns": b - 10 * s, "to_ns": b + 6 * s, "limit": 100, "page": None}),
        ("host_last_60s",     {"kind": "logs", "node": node, "from_ns": b - 60 * s, "to_ns": b + 6 * s, "limit": 1000, "page": None}),
        ("text_last_60s",     {"kind": "logs", "from_ns": b - 60 * s, "to_ns": b + 6 * s, "contains": "zq9", "limit": 100, "page": None}),
        ("fleet_last_15s",    {"kind": "metrics", "name": "sim.metric.3", "from_ns": b - 15 * s, "to_ns": b + 6 * s, "limit": 10000, "page": None}),
        ("rate_last_60s",     {"kind": "rate", "name": "sim.metric.0", "from_ns": b - 60 * s, "to_ns": b + 6 * s}),
        ("logs_limit50_full", {"kind": "logs", "from_ns": max(0, a - 60 * s), "to_ns": b + 6 * s, "limit": 50, "page": None}),
    ]

def bench(q, a, b, node, reps=10):
    out = {}
    for name, body in shapes(a, b, node):
        ts = []
        for _ in range(reps):
            t = time.perf_counter(); st, ans = q.post(body); ts.append((time.perf_counter() - t) * 1000)
            assert st == 200, (name, st, ans)
        out[name] = {"p50_ms": round(pct(ts, 50), 2), "min_ms": round(min(ts), 2), "max_ms": round(max(ts), 2), "rows": len(ans.get("rows", [])), "complete": ans.get("complete")}
    return out

def random_queries(a, b, labels, n=300, seed=11):
    rng = random.Random(seed); s = 1_000_000_000; out = []
    for _ in range(n):
        frm = rng.randint(a - 10 * s, b + 5 * s); to = frm + int(rng.uniform(0.5, 120) * s)
        node = rng.choice([None, None, rng.choice(labels), f"node-{rng.randrange(1000):04d}", "node-9999"])
        kind = rng.choice(["logs", "logs", "metrics", "rate"])
        body = {"kind": kind, "from_ns": frm, "to_ns": to}
        if node: body["node"] = node
        if kind == "logs":
            body.update({"limit": rng.choice([1, 10, 100, 1000]), "page": None})
            if rng.random() < 0.3: body["contains"] = rng.choice(["R", "zq9", "gap"])
        elif kind == "metrics":
            body.update({"name": f"sim.metric.{rng.randrange(32)}", "limit": rng.choice([1, 10, 100, 10000]), "page": None})
        else:
            body["name"] = f"sim.metric.{rng.randrange(32)}"
        out.append(body)
    return out

def answer_set(q, queries):
    got = []
    for body in queries:
        st, ans = q.post(body); entry = {"first": [st, ans], "pages": []}
        nxt = ans.get("next_page") if st == 200 else None; hops = 0
        while nxt and hops < 3:
            st2, ans2 = q.post(dict(body, page=nxt)); entry["pages"].append([st2, ans2])
            nxt = ans2.get("next_page") if st2 == 200 else None; hops += 1
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

path = ROOT / "tailbench.json"
result = json.load(open(path)) if path.exists() else {}
try:
    for kind in TAILS:
        tail = gen_tail(kind); size = tail.stat().st_size
        print(f"== {kind}: tail {size / MIB:.1f} MiB", flush=True)
        node = "node-0007"
        r = {"tail_bytes": size, "node": node}
        # stock
        state = make_state(f"{kind}-stock", tail)
        p, q, ready, logp = server("stock", state)
        m0 = status(p.pid); t = time.perf_counter(); a, b = window(q); first_ms = (time.perf_counter() - t) * 1000; m1 = status(p.pid)
        labels = sorted(set())
        st, ans = q.post({"kind": "logs", "from_ns": 0, "to_ns": 2**63, "limit": 10000, "page": None}); labels = sorted({row["node"] for row in ans["rows"]}) or [node]
        if kind == "needle": node = "node-0077"
        r["stock"] = {"ready_s": ready, "mem_after_replay_MiB": m0, "first_query_ms": round(first_ms, 1), "mem_after_first_query_MiB": m1, "shapes": bench(q, a, b, node)}
        r["stock"]["mem_after_shapes_MiB"] = status(p.pid)
        queries = random_queries(a, b, labels)
        t = time.perf_counter(); stock_answers = answer_set(q, queries); r["stock"]["random_set_s"] = round(time.perf_counter() - t, 2)
        r["stock"]["mem_after_random_MiB"] = status(p.pid); stop(p)
        # index
        state = make_state(f"{kind}-index", tail)
        p, q, ready, logp = server("proto", state, "index")
        m0 = status(p.pid); t = time.perf_counter(); window(q); first_ms = (time.perf_counter() - t) * 1000; m1 = status(p.pid)
        r["index"] = {"ready_s": ready, "mem_after_replay_MiB": m0, "first_query_ms": round(first_ms, 1), "mem_after_first_query_MiB": m1, "shapes": bench(q, a, b, node)}
        r["index"]["mem_after_shapes_MiB"] = status(p.pid)
        t = time.perf_counter(); index_answers = answer_set(q, queries); r["index"]["random_set_s"] = round(time.perf_counter() - t, 2)
        r["index"]["mem_after_random_MiB"] = status(p.pid); stop(p)
        lines = index_lines(logp)
        r["index"]["build"] = lines[0] if lines else None
        r["index"]["entries"] = int(lines[-1]["entries"]) if lines else None
        r["index"]["index_bytes"] = int(lines[-1]["bytes"]) if lines else None
        r["index"]["bytes_per_entry"] = round(int(lines[-1]["bytes"]) / max(1, int(lines[-1]["entries"])), 1) if lines else None
        r["index"]["max_selected"] = max(int(l["selected"]) for l in lines) if lines else None
        r["differential"] = {"queries": len(queries), "mismatches": diff(stock_answers, index_answers, queries)}
        result[kind] = r
        print(f"  tail {size/MIB:.0f} MiB  entries {r['index']['entries']}  index {r['index']['index_bytes']/MIB if r['index']['index_bytes'] else 0:.1f} MiB ({r['index']['bytes_per_entry']} B/entry)  build {r['index']['build']['extend_ms'] if r['index']['build'] else '?'} ms", flush=True)
        print(f"  first query: stock {r['stock']['first_query_ms']:.0f} ms  index {r['index']['first_query_ms']:.0f} ms;  HWM after first: stock {m1['VmHWM']} MiB (replay {r['stock']['mem_after_replay_MiB']['VmHWM']})  index {r['index']['mem_after_first_query_MiB']['VmHWM']} MiB (replay {r['index']['mem_after_replay_MiB']['VmHWM']})", flush=True)
        for k in r["stock"]["shapes"]:
            print(f"   {k:18} stock {r['stock']['shapes'][k]['p50_ms']:9.2f}  index {r['index']['shapes'][k]['p50_ms']:9.2f}  rows {r['stock']['shapes'][k]['rows']}/{r['index']['shapes'][k]['rows']}", flush=True)
        print(f"  random set: stock {r['stock']['random_set_s']} s  index {r['index']['random_set_s']} s;  mismatches {len(r['differential']['mismatches'])}", flush=True)
        json.dump(result, open(path, "w"), indent=1)

    # rotation and sealing while the index serves queries
    if "steady" in TAILS:
        tail = gen_tail("steady"); state = make_state("steady-rotate", tail)
        p, q, ready, logp = server("proto", state, "index", file_bytes=65536)
        a, b = window(q)
        import urllib.request
        ctx = ssl.create_default_context(cafile=str(ROOT / "ca.pem"))
        req = urllib.request.Request(f"https://127.0.0.1:{q.port}/v1/admin/nodes", data=json.dumps({"name": "kicker", "metric_interval_s": 15}).encode(), method="POST",
                                     headers={"authorization": f"Bearer {TOKEN}", "content-type": "application/json"})
        with urllib.request.urlopen(req, context=ctx, timeout=30) as resp: tok = json.loads(resp.read())["token"]
        (ROOT / "kick-tokens").write_text(tok + "\n")
        errors, counts, stop_flag = [], {"answers": 0}, threading.Event()
        def hammer():
            qq = Q(q.port)
            while not stop_flag.is_set():
                for name, body in shapes(a, b, "node-0007")[:6]:
                    try:
                        st, ans = qq.post(body)
                        if st != 200 or not ans.get("complete"): errors.append((name, st, str(ans)[:120]))
                        counts["answers"] += 1
                    except Exception as e:
                        errors.append((name, "exception", str(e)[:120])); qq.conn = None
        th = threading.Thread(target=hammer, daemon=True); th.start()
        sim = subprocess.Popen([str(BIN.parent.parent / "e2e" / "nonexistent")] if False else [str(SB.parent.parent.parent / "release" / "examples" / "spindle_sim"), "--server-url", f"https://127.0.0.1:{q.port}", "--ca", str(ROOT / "ca.pem"),
                                "--tokens", str(ROOT / "kick-tokens"), "--seed", "0xA11FA001", "--seconds", "12", "--workers", "1", "--out", str(ROOT / "kick")], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        kids.append(sim); sim.wait(timeout=120)
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline and (list((state / "journal").glob("sealed-*")) or not list((state / "segments").glob("seg-*"))):
            time.sleep(0.5)
        time.sleep(3); stop_flag.set(); th.join(timeout=30)
        lines = index_lines(logp)
        result["rotation"] = {"answers": counts["answers"], "errors": errors[:10], "error_count": len(errors), "segments": len(list((state / "segments").glob("seg-*"))),
                              "sealed_left": len(list((state / "journal").glob("sealed-*"))), "entries_first": int(lines[0]["entries"]) if lines else None,
                              "entries_max": max(int(l["entries"]) for l in lines) if lines else None, "entries_last": int(lines[-1]["entries"]) if lines else None}
        stop(p)
        print("rotation:", {k: v for k, v in result["rotation"].items() if k != "errors"}, flush=True)
        json.dump(result, open(path, "w"), indent=1)
finally:
    for p in kids:
        if p.poll() is None: p.kill(); p.wait(timeout=10)
