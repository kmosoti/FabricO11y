"""One registered soak trial: 100 simulated identities for 5,460 s against one
real fabric-server, with sealing, periodic queries and management throughout.

Runs inside a runner-owned target/alpha-* directory. Protocol:
docs/experiments/benchmarks/soak-protocol.md.

Measured per 600 s window after a 60 s warmup: durable ACK latency, server
RSS and CPU; over the run: backlog, query latency and completeness,
management success, sealing progress, live bytes, and delivery-oracle
correctness with bytes projected to SHA-256. The gates are decided by
`evaluate`, a pure function with its own negative controls.
"""

import argparse
import base64
import hashlib
import json
import random
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import delivery_oracle  # noqa: E402
from delivery_faults import free_port, make_certs  # noqa: E402
from fleet_tier import AdminClient, cpu_s, percentile, status_kib  # noqa: E402
from workload import SEEDS  # noqa: E402

IDENTITIES = 100
WARMUP = 60
WINDOW = 600
WINDOWS = 9
SECONDS = WARMUP + WINDOW * WINDOWS
QUERY_EVERY = 10
MANAGE_EVERY = 60
SAMPLE_EVERY = 5
SEAL_WAIT = 300
RSS_LIMIT_KIB = 2 * 1024 * 1024
CHILDREN = []


def evaluate(m):
    """The registered decision rule over one trial's measurements `m`."""
    windows = m["ack_ms_by_window"]
    rss = m["rss_kib_by_window"]
    rss_first = percentile(rss[0], 0.5) if rss and rss[0] else None
    rss_last = percentile(rss[-1], 0.5) if rss and rss[-1] else None
    queries = m["query_s"]
    return {
        "oracle": m["oracle_passed"],
        "exits": m["sim_exit"] == 0 and m["server_exit"] == 0,
        "ack_p99_le_1s_every_window": len(windows) == WINDOWS
        and all(w and percentile(w, 0.99) <= 1000 for w in windows),
        "no_growing_backlog": bool(m["backlog"])
        and m["backlog"][-1] <= m["backlog"][0] + IDENTITIES,
        "server_rss_le_2gib": m["server_vmhwm_kib"] <= RSS_LIMIT_KIB,
        "no_rss_growth": rss_first is not None and rss_last is not None
        and rss_last <= 2 * rss_first + 64 * 1024,
        "queries_complete": m["query_failed"] == 0 and m["query_incomplete"] == 0 and bool(queries),
        "query_p99_le_2s": bool(queries) and percentile(queries, 0.99) <= 2.0,
        "management_ok": m["management_failed"] == 0 and m["management_ok"] > 0,
        "sealing_caught_up": m["sealed_left"] == 0 and m["segments"] > 0,
    }


def main():
    try:
        return trial()
    finally:
        for child in CHILDREN:
            if child.poll() is None:
                child.kill()
                child.wait(timeout=10)


def trial():
    parser = argparse.ArgumentParser()
    parser.add_argument("--seed", type=lambda v: int(v, 0), required=True)
    parser.add_argument("--bin-dir", required=True)
    parser.add_argument("--server-cpus", required=True)
    parser.add_argument("--sim-cpus", required=True)
    # Two 30 s windows after a 10 s warmup, to try the harness; never a trial.
    parser.add_argument("--smoke", action="store_true")
    args = parser.parse_args()
    global WARMUP, WINDOW, WINDOWS, SECONDS
    if args.smoke:
        WARMUP, WINDOW, WINDOWS = 10, 30, 2
        SECONDS = WARMUP + WINDOW * WINDOWS
    if args.seed not in SEEDS:
        raise SystemExit("unregistered seed")
    root = Path.cwd().resolve()
    if (root / ".fabric-alpha-owned").read_text() != "fabric-alpha-runner-v1\n":
        raise SystemExit("soak trial must run inside a runner-owned directory")
    bins = Path(args.bin_dir).resolve(strict=True)
    rng = random.Random(args.seed)
    make_certs(root)
    port = free_port()
    admin_token = hashlib.sha256(f"admin:{args.seed}".encode()).hexdigest()
    (root / "admin-token").write_text(admin_token + "\n")
    server_conf = root / "server.conf"
    server_conf.write_text(
        f"listen=127.0.0.1:{port}\ntls_cert={root}/server.pem\ntls_key={root}/server.key\n"
        f"state_dir={root}/server-state\nadmin_token_file={root}/admin-token\n"
        f"journal_bytes=4294967296\njournal_file_bytes={64 * 1024 * 1024}\n")
    server = subprocess.Popen(["taskset", "-c", args.server_cpus, str(bins / "fabric-server"), "serve", str(server_conf)],
                              stdout=open(root / "server.log", "wb"), stderr=subprocess.STDOUT)
    CHILDREN.append(server)
    admin = AdminClient(port, root / "ca.pem", admin_token)
    deadline = time.monotonic() + 15
    while True:
        try:
            admin.call("GET")
            break
        except OSError:
            if time.monotonic() > deadline or server.poll() is not None:
                raise SystemExit("server did not start")
            time.sleep(0.1)
    tokens = [admin.call("POST", body={"name": f"sim{i:04d}", "metric_interval_s": 15})["token"]
              for i in range(IDENTITIES)]
    (root / "tokens").write_text("\n".join(tokens) + "\n")
    query = AdminClient(port, root / "ca.pem", admin_token)
    query.base = f"https://127.0.0.1:{port}/v1/admin/query"
    sim = subprocess.Popen(["taskset", "-c", args.sim_cpus, str(bins / "examples" / "spindle_sim"),
                            "--server-url", f"https://127.0.0.1:{port}", "--ca", str(root / "ca.pem"),
                            "--tokens", str(root / "tokens"), "--seed", hex(args.seed),
                            "--seconds", str(SECONDS), "--workers", str(IDENTITIES), "--out", str(root / "sim")],
                           stdout=open(root / "sim.log", "wb"), stderr=subprocess.STDOUT)
    CHILDREN.append(sim)
    began = time.monotonic()
    probes = {"query_s": [], "failed": 0, "incomplete": 0, "examples": []}
    management = {"ok": 0, "failed": 0}
    samples = []  # (seconds since start, VmRSS KiB, CPU seconds)

    def prober():
        while sim.poll() is None:
            started = time.monotonic()
            now = time.time_ns()
            body = {"kind": "logs", "node": f"sim{rng.randrange(IDENTITIES):04d}",
                    "from_ns": now - 30 * 10**9, "to_ns": now, "limit": 100}
            try:
                while True:
                    page = query.call("POST", body=body)
                    if not page.get("complete"):
                        probes["incomplete"] += 1
                    if not page.get("next_page"):
                        break
                    body["page"] = page["next_page"]
                probes["query_s"].append(time.monotonic() - started)
            except Exception as error:  # a failed probe is recorded, not hidden
                probes["failed"] += 1
                probes["examples"].append(repr(error)[:200])
            time.sleep(max(0.0, QUERY_EVERY - (time.monotonic() - started)))

    def manager():
        while sim.poll() is None:
            started = time.monotonic()
            try:
                admin.call("GET")
                admin.call("PUT", f"/sim{rng.randrange(IDENTITIES):04d}/config",
                           {"logs": [], "metric_interval_s": rng.choice([15, 30])})
                management["ok"] += 1
            except Exception:
                management["failed"] += 1
            time.sleep(max(0.0, MANAGE_EVERY - (time.monotonic() - started)))

    def sampler():
        while sim.poll() is None:
            samples.append((time.monotonic() - began, status_kib(server.pid, "VmRSS"), cpu_s(server.pid)))
            time.sleep(SAMPLE_EVERY)

    threads = [threading.Thread(target=f, daemon=True) for f in (prober, manager, sampler)]
    for t in threads:
        t.start()
    sim.wait()
    for t in threads:
        t.join(timeout=120)
    sim_exit = sim.returncode
    journal = root / "server-state" / "journal"
    seal_deadline = time.monotonic() + SEAL_WAIT
    while any(journal.glob("sealed-*.faj")) and time.monotonic() < seal_deadline:
        time.sleep(1)
    sealed_left = len(list(journal.glob("sealed-*.faj")))
    segments = len(list((root / "server-state" / "segments").glob("seg-*")))
    server_hwm = status_kib(server.pid, "VmHWM")
    server.send_signal(signal.SIGTERM)
    server_exit = server.wait(timeout=120)

    # Streamed: the dump of a soak is too large to hold in memory twice.
    transcript = root / "sim" / "transcript.jsonl"
    with open(root / "dump.err", "wb") as err, open(transcript, "a") as out:
        dump = subprocess.Popen([str(bins / "examples" / "server_dump"), str(server_conf)],
                                stdout=subprocess.PIPE, stderr=err, text=True)
        for line in dump.stdout:
            record = json.loads(line)
            record["bytes"] = base64.b64encode(hashlib.sha256(base64.b64decode(record["bytes"])).digest()).decode()
            out.write(json.dumps(record) + "\n")
        out.write('{"type": "end"}\n')
        if dump.wait(timeout=1800):
            raise SystemExit("server_dump failed; see dump.err")
    with open(transcript) as source:
        verdict = delivery_oracle.check(source)

    began_ns = json.loads((root / "sim" / "sim-summary.json").read_text())["began_unix_ns"]
    ack_ms = [[] for _ in range(WINDOWS)]
    created, acked = {}, {}
    with open(root / "sim" / "events.jsonl") as events:
        for line in events:
            e = json.loads(line)
            if e["e"] == "created":
                created[(e["id"], e["seq"])] = e["t"]
            elif e["e"] == "attempt" and e["kind"] == "ack":
                acked.setdefault((e["id"], e["seq"]), e["end"])
                offset = (e["start"] - began_ns) / 1e9 - WARMUP
                if 0 <= offset < WINDOW * WINDOWS:
                    ack_ms[int(offset // WINDOW)].append((e["end"] - e["start"]) / 1e6)
    # Backlog: created and not yet acknowledged, every 60 s from the end of
    # the warmup to 5 s before the end.
    by_created = sorted(created.values())
    by_acked = sorted(acked.get(k, 1 << 63) for k in created)
    backlog = []
    for second in list(range(WARMUP, SECONDS - 5, 60)) + [SECONDS - 5]:
        t = began_ns + second * 10**9
        made = _count_le(by_created, t)
        done = _count_le(by_acked, t)
        backlog.append(made - done)
    rss = [[] for _ in range(WINDOWS)]
    cpu = [[] for _ in range(WINDOWS)]
    for at, kib, seconds in samples:
        index = int((at - WARMUP) // WINDOW)
        if 0 <= index < WINDOWS:
            rss[index].append(kib)
            cpu[index].append(seconds)
    live_bytes = sum(p.stat().st_size for p in root.rglob("*") if p.is_file())
    measurements = {
        "oracle_passed": verdict.passed, "sim_exit": sim_exit, "server_exit": server_exit,
        "ack_ms_by_window": ack_ms, "backlog": backlog, "server_vmhwm_kib": server_hwm,
        "rss_kib_by_window": rss, "query_s": probes["query_s"], "query_failed": probes["failed"],
        "query_incomplete": probes["incomplete"], "management_ok": management["ok"],
        "management_failed": management["failed"], "sealed_left": sealed_left, "segments": segments,
    }
    gates = evaluate(measurements)
    summary = {
        "seed": args.seed, "identities": IDENTITIES, "seconds": SECONDS, "smoke": args.smoke,
        "passed": all(gates.values()),
        "gates": gates, "violations": [v.__dict__ for v in verdict.violations][:5],
        "server_cpus": args.server_cpus, "sim_cpus": args.sim_cpus,
        "sim_exit": sim_exit, "server_exit": server_exit,
        "batches_created": len(created), "batches_acked": len(acked),
        "windows": [{
            "from_s": WARMUP + i * WINDOW,
            "ack_ms_p50": percentile(ack_ms[i], 0.5), "ack_ms_p99": percentile(ack_ms[i], 0.99),
            "acks": len(ack_ms[i]),
            "rss_kib_p50": percentile(rss[i], 0.5), "rss_kib_max": max(rss[i]) if rss[i] else None,
            "server_cpu_s": round(cpu[i][-1] - cpu[i][0], 2) if len(cpu[i]) > 1 else None,
        } for i in range(WINDOWS)],
        "backlog_first": backlog[0] if backlog else None, "backlog_last": backlog[-1] if backlog else None,
        "backlog_max": max(backlog) if backlog else None,
        "query_n": len(probes["query_s"]), "query_p50_s": percentile(probes["query_s"], 0.5),
        "query_p99_s": percentile(probes["query_s"], 0.99), "query_failed": probes["failed"],
        "query_incomplete": probes["incomplete"], "query_error_examples": probes["examples"][:5],
        "management_ok": management["ok"], "management_failed": management["failed"],
        "sealed_left": sealed_left, "segments": segments,
        "server_vmhwm_kib": server_hwm, "live_bytes_end": live_bytes,
    }
    (root / "soak-summary.json").write_text(json.dumps(summary, sort_keys=True) + "\n")
    print(json.dumps(summary, sort_keys=True))
    return 0 if summary["passed"] else 1


def _count_le(ordered, value):
    """How many items of the sorted list `ordered` are at most `value`."""
    lo, hi = 0, len(ordered)
    while lo < hi:
        mid = (lo + hi) // 2
        if ordered[mid] <= value:
            lo = mid + 1
        else:
            hi = mid
    return lo


if __name__ == "__main__":
    sys.exit(main())
