"""Stress qualification: 5x burst, rejected credentials, malformed batches, and
concurrent management and queries, on 1,000 simulated identities.

Runs inside a runner-owned target/alpha-* directory. Protocol:
docs/experiments/benchmarks/alpha-phase5-stress-protocol.md. Bursts test
correctness and bounds, not steady latency.
"""

import argparse
import base64
import hashlib
import json
import os
import random
import signal
import ssl
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import delivery_oracle  # noqa: E402
from delivery_faults import free_port, make_certs  # noqa: E402
from fleet_tier import AdminClient, percentile  # noqa: E402
from workload import SEEDS  # noqa: E402

IDENTITIES = 1000
SECONDS = 180
BURST = (60, 80, 5)
CHILDREN = []


def status_kib(pid, key):
    for line in Path(f"/proc/{pid}/status").read_text().splitlines():
        if line.startswith(key + ":"):
            return int(line.split()[1])
    raise RuntimeError(key)


def post_batch(port, ctx, token, body):
    request = urllib.request.Request(f"https://127.0.0.1:{port}/v1/batches", data=body, method="POST",
                                     headers={"authorization": f"Bearer {token}",
                                              "content-type": "application/x-protobuf"})
    try:
        with urllib.request.urlopen(request, context=ctx, timeout=30) as response:
            return response.status
    except urllib.error.HTTPError as error:
        return error.code


def varint(n):
    out = bytearray()
    while True:
        byte = n & 0x7F
        n >>= 7
        out.append(byte | (0x80 if n else 0))
        if not n:
            return bytes(out)


def field(tag, wire, payload):
    return varint(tag << 3 | wire) + payload


def batch_bytes(node_id, sequence):
    """A minimal valid Batch: identity plus one collection gap."""
    gap = b"stress probe"
    return (field(1, 0, varint(1)) + field(2, 2, varint(16) + node_id) + field(3, 0, varint(1))
            + field(4, 0, varint(sequence)) + field(8, 2, varint(len(gap)) + gap))


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
    # CPU placement. The defaults are revision 1 (server 0-3, simulator the
    # rest); revision 2 (four-CPU host) passes 0-1 and 2-3.
    parser.add_argument("--server-cpus", default="0-3")
    parser.add_argument("--sim-cpus", default=None)
    args = parser.parse_args()
    if args.seed not in SEEDS:
        raise SystemExit("unregistered seed")
    root = Path.cwd().resolve()
    if (root / ".fabric-alpha-owned").read_text() != "fabric-alpha-runner-v1\n":
        raise SystemExit("stress trial must run inside a runner-owned directory")
    bins = Path(args.bin_dir).resolve(strict=True)
    cpus = os.cpu_count() or 1
    sim_cpus = args.sim_cpus or f"4-{cpus - 1}"
    rng = random.Random(args.seed)
    make_certs(root)
    port = free_port()
    admin_token = hashlib.sha256(f"admin:{args.seed}".encode()).hexdigest()
    (root / "admin-token").write_text(admin_token + "\n")
    server_conf = root / "server.conf"
    server_conf.write_text(
        f"listen=127.0.0.1:{port}\ntls_cert={root}/server.pem\ntls_key={root}/server.key\n"
        f"state_dir={root}/server-state\nadmin_token_file={root}/admin-token\njournal_bytes=4294967296\n")
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
    tokens = [admin.call("POST", body={"name": f"sim{i:04d}"})["token"] for i in range(IDENTITIES)]
    (root / "tokens").write_text("\n".join(tokens) + "\n")
    # Identities used only to probe rejection; half are revoked before use.
    probes = [admin.call("POST", body={"name": f"probe{i}"})["token"] for i in range(10)]
    for i in range(5):
        admin.call("POST", f"/probe{i}/revoke")
    ctx = ssl.create_default_context(cafile=str(root / "ca.pem"))
    query = AdminClient(port, root / "ca.pem", admin_token)
    query.base = f"https://127.0.0.1:{port}/v1/admin/query"
    sim = subprocess.Popen(["taskset", "-c", sim_cpus, str(bins / "examples" / "spindle_sim"),
                            "--server-url", f"https://127.0.0.1:{port}", "--ca", str(root / "ca.pem"),
                            "--tokens", str(root / "tokens"), "--seed", hex(args.seed),
                            "--seconds", str(SECONDS), "--workers", "128", "--out", str(root / "sim"),
                            "--burst-from", str(BURST[0]), "--burst-to", str(BURST[1]),
                            "--burst-factor", str(BURST[2])],
                           stdout=open(root / "sim.log", "wb"), stderr=subprocess.STDOUT)
    CHILDREN.append(sim)
    rejected_ids = []
    rejections = {"revoked": [], "unknown": [], "malformed": []}
    management = {"ok": 0, "failed": 0, "query_s": []}
    peak_rss = [0]

    def attacker():
        for n in range(200):
            node_id = os.urandom(16)
            rejected_ids.append(node_id.hex())
            body = batch_bytes(node_id, 1)
            rejections["revoked"].append(post_batch(port, ctx, probes[n % 5], body))
            rejections["unknown"].append(post_batch(port, ctx, hashlib.sha256(os.urandom(8)).hexdigest(), body))
            rejections["malformed"].append(post_batch(port, ctx, probes[5 + n % 5], b"\xff" * (n + 1)))
            time.sleep(0.2)

    def manager():
        while sim.poll() is None:
            try:
                admin.call("GET")
                admin.call("PUT", f"/sim{rng.randrange(IDENTITIES):04d}/config",
                           {"logs": [], "metric_interval_s": rng.choice([15, 30])})
                started = time.monotonic()
                now = time.time_ns()
                query.call("POST", body={"kind": "logs", "node": f"sim{rng.randrange(IDENTITIES):04d}",
                                         "from_ns": now - 30 * 10**9, "to_ns": now, "limit": 100})
                management["query_s"].append(time.monotonic() - started)
                management["ok"] += 1
            except Exception:
                management["failed"] += 1
            time.sleep(0.5)

    def sampler():
        while sim.poll() is None:
            peak_rss[0] = max(peak_rss[0], status_kib(server.pid, "VmRSS"))
            time.sleep(0.2)

    threads = [threading.Thread(target=f, daemon=True) for f in (attacker, manager, sampler)]
    for t in threads:
        t.start()
    sim.wait()
    for t in threads:
        t.join(timeout=60)
    sim_exit = sim.returncode
    server_hwm = status_kib(server.pid, "VmHWM")
    server.send_signal(signal.SIGTERM)
    server_exit = server.wait(timeout=120)

    dump = subprocess.run([str(bins / "examples" / "server_dump"), str(server_conf)],
                          capture_output=True, text=True, timeout=1200, check=True)
    recovered = [json.loads(l) for l in dump.stdout.splitlines() if l]
    committed_rejected = sum(1 for r in recovered if r["node_id"] in set(rejected_ids))
    transcript = root / "sim" / "transcript.jsonl"
    with open(transcript, "a") as out:
        for record in recovered:
            record["bytes"] = base64.b64encode(hashlib.sha256(base64.b64decode(record["bytes"])).digest()).decode()
            out.write(json.dumps(record) + "\n")
        out.write('{"type": "end"}\n')
    with open(transcript) as source:
        verdict = delivery_oracle.check(source)

    began_ns = json.loads((root / "sim" / "sim-summary.json").read_text())["began_unix_ns"]
    created, acked = {}, {}
    # Reported, not gated: ACK latency (request start to ACK) for attempts
    # started in the 20 s before the burst and during it.
    ack_ms = {"before_burst": [], "burst": []}
    with open(root / "sim" / "events.jsonl") as events:
        for line in events:
            e = json.loads(line)
            if e["e"] == "created":
                created[(e["id"], e["seq"])] = e["t"]
            elif e["e"] == "attempt" and e["kind"] == "ack":
                acked.setdefault((e["id"], e["seq"]), e["end"])
                second = (e["start"] - began_ns) / 1e9
                if BURST[0] - 20 <= second < BURST[0]:
                    ack_ms["before_burst"].append((e["end"] - e["start"]) / 1e6)
                elif BURST[0] <= second < BURST[1]:
                    ack_ms["burst"].append((e["end"] - e["start"]) / 1e6)
    backlog = {}
    for second in (BURST[0] - 5, BURST[1] - 1, BURST[1] + 20, SECONDS - 5):
        t = began_ns + second * 10**9
        backlog[second] = sum(1 for k, c in created.items() if c <= t and acked.get(k, 1 << 63) > t)
    gates = {
        "oracle": verdict.passed,
        "exits": sim_exit == 0 and server_exit == 0,
        "revoked_rejected": all(code == 401 for code in rejections["revoked"]),
        "unknown_rejected": all(code == 401 for code in rejections["unknown"]),
        "malformed_rejected": all(code in (400, 413) for code in rejections["malformed"]),
        "no_rejected_batch_committed": committed_rejected == 0,
        "backlog_recovers_after_burst": backlog[SECONDS - 5] <= backlog[BURST[0] - 5] + IDENTITIES,
        "server_rss_le_2gib": server_hwm <= 2 * 1024 * 1024,
        "management_ok": management["failed"] == 0 and management["ok"] > 0,
    }
    summary = {
        "seed": args.seed, "passed": all(gates.values()), "gates": gates,
        "violations": [v.__dict__ for v in verdict.violations][:5],
        "batches_created": len(created), "batches_acked": len(acked), "backlog": backlog,
        "rejections": {k: sorted(set(v)) for k, v in rejections.items()},
        "rejection_counts": {k: len(v) for k, v in rejections.items()},
        "committed_rejected": committed_rejected,
        "management_ok": management["ok"], "management_failed": management["failed"],
        "concurrent_query_p99_s": percentile(management["query_s"], 0.99),
        "ack_ms": {window: {"n": len(v), "p50": percentile(v, 0.5), "p99": percentile(v, 0.99),
                            "max": max(v) if v else None} for window, v in ack_ms.items()},
        "server_vmhwm_kib": server_hwm, "server_rss_peak_sampled_kib": peak_rss[0],
        "sim_exit": sim_exit, "server_exit": server_exit,
        "server_cpus": args.server_cpus, "sim_cpus": sim_cpus,
    }
    (root / "stress-summary.json").write_text(json.dumps(summary, sort_keys=True) + "\n")
    print(json.dumps(summary, sort_keys=True))
    return 0 if summary["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
