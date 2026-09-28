"""One fleet-tier trial: a real fabric-server and N simulated node identities.

Runs inside a runner-owned target/alpha-* directory. The server is pinned to
logical CPUs 0-3 (the contract's four-CPU server budget) and the simulator to
the remaining CPUs. Registered protocol:
docs/experiments/benchmarks/alpha-phase3-fleet-protocol.md.

Measured: durable ACK latency (request start to ack), delivery latency
(batch creation to ack), backlog, server CPU and peak RSS, configuration
apply latency after one change to every identity at t=60 s, all live bytes,
and delivery-oracle correctness with bytes projected to SHA-256.
"""

import argparse
import base64
import hashlib
import json
import os
import signal
import ssl
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import delivery_oracle  # noqa: E402
from delivery_faults import free_port, make_certs  # noqa: E402
from workload import SEEDS, TIERS  # noqa: E402

WARMUP = 15
MEASURED = 120
TOTAL = WARMUP + MEASURED
APPLY_AT = 60
TICKS = os.sysconf("SC_CLK_TCK")
CHILDREN = []


def cpu_s(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rpartition(") ")[2].split()
    return (int(fields[11]) + int(fields[12])) / TICKS


def status_kib(pid, key):
    for line in Path(f"/proc/{pid}/status").read_text().splitlines():
        if line.startswith(key + ":"):
            return int(line.split()[1])
    raise RuntimeError(key)


def percentile(values, q):
    if not values:
        return None
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(q * len(ordered)))]


class AdminClient:
    def __init__(self, port, ca, token):
        self.base = f"https://127.0.0.1:{port}/v1/admin/nodes"
        self.ctx = ssl.create_default_context(cafile=str(ca))
        self.auth = f"Bearer {token}"

    def call(self, method, tail="", body=None):
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(self.base + tail, data=data, method=method,
                                         headers={"authorization": self.auth,
                                                  "content-type": "application/json"})
        with urllib.request.urlopen(request, context=self.ctx, timeout=30) as response:
            return json.loads(response.read() or b"null")


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
    parser.add_argument("--tier", type=int, required=True)
    parser.add_argument("--seed", type=lambda v: int(v, 0), required=True)
    parser.add_argument("--bin-dir", required=True)
    args = parser.parse_args()
    if args.tier not in TIERS or args.seed not in SEEDS:
        raise SystemExit("unregistered tier or seed")
    root = Path.cwd().resolve()
    if (root / ".fabric-alpha-owned").read_text() != "fabric-alpha-runner-v1\n":
        raise SystemExit("fleet trial must run inside a runner-owned directory")
    bins = Path(args.bin_dir).resolve(strict=True)
    cpus = os.cpu_count() or 1
    if cpus < 6:
        raise SystemExit("fleet trial needs at least 6 logical CPUs")
    make_certs(root)
    port = free_port()
    admin_token = hashlib.sha256(f"admin:{args.seed}".encode()).hexdigest()
    (root / "admin-token").write_text(admin_token + "\n")
    server_conf = root / "server.conf"
    server_conf.write_text(
        f"listen=127.0.0.1:{port}\ntls_cert={root}/server.pem\ntls_key={root}/server.key\n"
        f"state_dir={root}/server-state\nadmin_token_file={root}/admin-token\njournal_bytes=4294967296\n")
    server = subprocess.Popen(["taskset", "-c", "0-3", str(bins / "fabric-server"), "serve", str(server_conf)],
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
    tokens = []
    enroll_started = time.monotonic()
    for i in range(args.tier):
        tokens.append(admin.call("POST", body={"name": f"sim{i:04d}", "metric_interval_s": 15})["token"])
    enroll_s = time.monotonic() - enroll_started
    (root / "tokens").write_text("\n".join(tokens) + "\n")
    sim_out = root / "sim"
    workers = min(128, args.tier)
    sim = subprocess.Popen(["taskset", "-c", f"4-{cpus - 1}", str(bins / "examples" / "alpha_node_sim"),
                            "--server-url", f"https://127.0.0.1:{port}", "--ca", str(root / "ca.pem"),
                            "--tokens", str(root / "tokens"), "--seed", hex(args.seed),
                            "--seconds", str(TOTAL), "--workers", str(workers), "--out", str(sim_out)],
                           stdout=open(root / "sim.log", "wb"), stderr=subprocess.STDOUT)
    CHILDREN.append(sim)
    began = time.monotonic()
    cpu_start = None
    peak_server_rss = 0
    puts = {}
    applied_before = False
    while sim.poll() is None:
        now = time.monotonic() - began
        peak_server_rss = max(peak_server_rss, status_kib(server.pid, "VmRSS"))
        if cpu_start is None and now >= WARMUP:
            cpu_start = cpu_s(server.pid)
        if not applied_before and now >= APPLY_AT:
            applied_before = True
            for i in range(args.tier):
                admin.call("PUT", f"/sim{i:04d}/config", {"logs": [], "metric_interval_s": 30})
                puts[i] = time.time_ns()
        if now >= TOTAL and "cpu_end" not in locals():
            cpu_end = cpu_s(server.pid)
        time.sleep(0.2)
    if "cpu_end" not in locals():
        cpu_end = cpu_s(server.pid)
    sim_exit = sim.returncode
    server_hwm = status_kib(server.pid, "VmHWM")
    inventory = admin.call("GET")["nodes"]
    server.send_signal(signal.SIGTERM)
    server_exit = server.wait(timeout=60)
    dump = subprocess.run([str(bins / "examples" / "server_dump"), str(server_conf)],
                          capture_output=True, text=True, timeout=600)
    if dump.returncode:
        raise SystemExit(f"server_dump failed: {dump.stderr}")

    transcript = sim_out / "transcript.jsonl"
    with open(transcript, "a") as out:
        for line in dump.stdout.splitlines():
            record = json.loads(line)
            record["bytes"] = base64.b64encode(hashlib.sha256(base64.b64decode(record["bytes"])).digest()).decode()
            out.write(json.dumps(record) + "\n")
        out.write('{"type": "end"}\n')
    with open(transcript) as source:
        verdict = delivery_oracle.check(source)

    began_ns = json.loads((sim_out / "sim-summary.json").read_text())["began_unix_ns"]
    window = (began_ns + WARMUP * 10**9, began_ns + TOTAL * 10**9)
    ack_ms, delivery_ms, applied = [], [], {}
    created, acked_at = {}, {}
    with open(sim_out / "events.jsonl") as events:
        for line in events:
            e = json.loads(line)
            if e["e"] == "created":
                created[(e["id"], e["seq"])] = e["t"]
            elif e["e"] == "attempt" and e["kind"] == "ack":
                key = (e["id"], e["seq"])
                if key not in acked_at:
                    acked_at[key] = e["end"]
                if window[0] <= e["start"] <= window[1]:
                    ack_ms.append((e["end"] - e["start"]) / 1e6)
                    delivery_ms.append((e["end"] - e["created"]) / 1e6)
            elif e["e"] == "applied" and e["id"] in puts and e["t"] >= puts[e["id"]]:
                applied.setdefault(e["id"], e["t"])
    apply_s = [(applied[i] - puts[i]) / 1e9 for i in puts if i in applied]
    # Backlog: batches created and not yet acknowledged, every 5 s of the window.
    backlog = []
    for t in range(window[0], window[1] + 1, 5 * 10**9):
        backlog.append(sum(1 for k, c in created.items() if c <= t and acked_at.get(k, 1 << 63) > t))
    live_bytes = sum(p.stat().st_size for p in root.rglob("*") if p.is_file())
    p99 = percentile(ack_ms, 0.99)
    gates = {
        "oracle": verdict.passed,
        "exits": sim_exit == 0 and server_exit == 0,
        "ack_p99_le_1s": p99 is not None and p99 <= 1000,
        "no_growing_backlog": bool(backlog) and backlog[-1] <= backlog[0] + args.tier,
        "apply_all_le_30s": len(apply_s) == args.tier and max(apply_s) <= 30,
        "server_rss_le_2gib": server_hwm <= 2 * 1024 * 1024,
    }
    summary = {
        "tier": args.tier, "seed": args.seed, "passed": all(gates.values()), "gates": gates,
        "violations": [v.__dict__ for v in verdict.violations][:5],
        "enroll_s": round(enroll_s, 2), "sim_exit": sim_exit, "server_exit": server_exit,
        "batches_created": len(created), "batches_acked": len(acked_at),
        "ack_ms_p50": percentile(ack_ms, 0.5), "ack_ms_p99": p99,
        "delivery_ms_p50": percentile(delivery_ms, 0.5), "delivery_ms_p99": percentile(delivery_ms, 0.99),
        "backlog_first": backlog[0] if backlog else None, "backlog_last": backlog[-1] if backlog else None,
        "backlog_max": max(backlog) if backlog else None,
        "apply_s_p50": percentile(apply_s, 0.5), "apply_s_p99": percentile(apply_s, 0.99),
        "apply_s_max": max(apply_s) if apply_s else None, "applied_identities": len(apply_s),
        "inventory_applied_eq_desired": sum(1 for n in inventory if n["applied_revision"] == n["desired_revision"]),
        "server_cpu_s_measured": round(cpu_end - (cpu_start or cpu_end), 2),
        "server_vmhwm_kib": server_hwm, "server_rss_peak_sampled_kib": peak_server_rss,
        "offered_records_per_s": args.tier * (2 + 32 / 15),
        "live_bytes_end": live_bytes,
    }
    (root / "fleet-summary.json").write_text(json.dumps(summary, sort_keys=True) + "\n")
    print(json.dumps(summary, sort_keys=True))
    return 0 if summary["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
