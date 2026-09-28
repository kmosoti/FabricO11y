"""One registered phase-4 trial: 1,000 identities for 250 s, freshness probes,
then the registered queries, graded by the frozen query oracle.

Runs inside a runner-owned target/alpha-* directory. Protocol:
docs/experiments/benchmarks/alpha-phase4-history-protocol.md.
"""

import argparse
import hashlib
import json
import os
import random
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import query_oracle  # noqa: E402
from delivery_faults import free_port, make_certs  # noqa: E402
from fleet_tier import AdminClient, percentile  # noqa: E402
from workload import SEEDS, entropy_body  # noqa: E402

IDENTITIES = 1000
SECONDS = 250
WARMUP = 15
REPEATS = 20
CHILDREN = []


def dir_bytes(path: Path) -> int:
    return sum(p.stat().st_size for p in path.rglob("*") if p.is_file()) if path.exists() else 0


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
    parser.add_argument("--mode", choices=["segment", "journal"], required=True)
    parser.add_argument("--bin-dir", required=True)
    # Registered trials use the default; a shorter run is a smoke test only.
    parser.add_argument("--seconds", type=int, default=SECONDS)
    args = parser.parse_args()
    if args.seed not in SEEDS:
        raise SystemExit("unregistered seed")
    root = Path.cwd().resolve()
    if (root / ".fabric-alpha-owned").read_text() != "fabric-alpha-runner-v1\n":
        raise SystemExit("history trial must run inside a runner-owned directory")
    bins = Path(args.bin_dir).resolve(strict=True)
    cpus = os.cpu_count() or 1
    rng = random.Random(args.seed)
    make_certs(root)
    port = free_port()
    admin_token = hashlib.sha256(f"admin:{args.seed}".encode()).hexdigest()
    (root / "admin-token").write_text(admin_token + "\n")
    file_bytes = 64 * 1024 * 1024 if args.mode == "segment" else 4 * 1024 * 1024 * 1024
    server_conf = root / "server.conf"
    server_conf.write_text(
        f"listen=127.0.0.1:{port}\ntls_cert={root}/server.pem\ntls_key={root}/server.key\n"
        f"state_dir={root}/server-state\nadmin_token_file={root}/admin-token\n"
        f"journal_bytes=4294967296\njournal_file_bytes={file_bytes}\n")
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
    tokens = [admin.call("POST", body={"name": f"sim{i:04d}"})["token"] for i in range(IDENTITIES)]
    (root / "tokens").write_text("\n".join(tokens) + "\n")
    query_url = AdminClient(port, root / "ca.pem", admin_token)
    query_url.base = f"https://127.0.0.1:{port}/v1/admin/query"

    def ask(body):
        """Every page of one query; returns (seconds, pages)."""
        started = time.monotonic()
        pages = []
        body = dict(body)
        while True:
            page = query_url.call("POST", body=body)
            pages.append(page)
            if not page.get("next_page"):
                return time.monotonic() - started, pages
            body["page"] = page["next_page"]

    sim = subprocess.Popen(["taskset", "-c", f"4-{cpus - 1}", str(bins / "examples" / "spindle_sim"),
                            "--server-url", f"https://127.0.0.1:{port}", "--ca", str(root / "ca.pem"),
                            "--tokens", str(root / "tokens"), "--seed", hex(args.seed),
                            "--seconds", str(args.seconds), "--workers", "128", "--out", str(root / "sim")],
                           stdout=open(root / "sim.log", "wb"), stderr=subprocess.STDOUT)
    CHILDREN.append(sim)
    began = time.monotonic()
    freshness = []
    probe_errors = []

    def prober():
        while sim.poll() is None:
            node = f"sim{rng.randrange(IDENTITIES):04d}"
            now = time.time_ns()
            try:
                _, pages = ask({"kind": "logs", "node": node, "from_ns": now - 10 * 10**9,
                                "to_ns": now + 10**9, "limit": 10})
                received = time.time_ns()
                newest = pages[0]["freshness"].get(node)
                if newest is not None and time.monotonic() - began >= WARMUP:
                    freshness.append((received - newest) / 1e9)
            except Exception as error:  # a probe failure is recorded, not hidden
                probe_errors.append(repr(error)[:200])
            time.sleep(max(0.0, 1.0 - (time.time_ns() - now) / 1e9))

    probe = threading.Thread(target=prober, daemon=True)
    probe.start()
    sim.wait()
    probe.join(timeout=30)
    sim_exit = sim.returncode
    if args.mode == "segment":
        deadline = time.monotonic() + 300
        while any((root / "server-state" / "journal").glob("sealed-*.faj")):
            if time.monotonic() > deadline:
                raise SystemExit("sealing did not finish")
            time.sleep(1)
    summary_sim = json.loads((root / "sim" / "sim-summary.json").read_text())
    t0 = summary_sim["began_unix_ns"]
    span = args.seconds * 10**9

    def window():
        start = t0 + rng.randrange(0, span - 60 * 10**9)
        return start, start + 60 * 10**9

    kinds = []
    for _ in range(REPEATS):
        node = rng.randrange(IDENTITIES)
        a, b = window()
        kinds.append(("host_logs", {"kind": "logs", "node": f"sim{node:04d}", "from_ns": a, "to_ns": b, "limit": 1000}))
        a, b = window()
        tick = ((a - t0) // 10**9 + 10) * 2 + 1  # an odd (high-entropy) tick inside the window
        needle = entropy_body(args.seed, rng.randrange(IDENTITIES), tick)[100:112]
        kinds.append(("text_search", {"kind": "logs", "from_ns": a, "to_ns": b, "contains": needle, "limit": 100}))
        kinds.append(("metric_history", {"kind": "metrics", "node": f"sim{rng.randrange(IDENTITIES):04d}",
                                         "name": "sim.metric.7", "from_ns": t0, "to_ns": t0 + span, "limit": 1000}))
        a, b = window()
        kinds.append(("fleet_metrics", {"kind": "metrics", "name": "sim.metric.3", "from_ns": a, "to_ns": b, "limit": 10000}))
        kinds.append(("rate", {"kind": "rate", "name": "sim.metric.0", "from_ns": t0, "to_ns": t0 + span}))
    rng.shuffle(kinds)
    latency, graded_answers = {}, {}
    for kind, body in kinds:
        seconds, pages = ask(body)
        latency.setdefault(kind, []).append(seconds)
        graded_answers.setdefault(kind, (body, pages))
    server_hwm = int([l for l in Path(f"/proc/{server.pid}/status").read_text().splitlines()
                      if l.startswith("VmHWM:")][0].split()[1])
    state = root / "server-state"
    bytes_at_rest = {"journal": dir_bytes(state / "journal"), "segments": dir_bytes(state / "segments"),
                     "checkpoint": (state / "streams.json").stat().st_size if (state / "streams.json").exists() else 0}
    server.send_signal(signal.SIGTERM)
    server_exit = server.wait(timeout=120)

    dump = subprocess.run([str(bins / "examples" / "server_dump"), str(server_conf), "--records"],
                          capture_output=True, text=True, timeout=1200)
    if dump.returncode:
        raise SystemExit(f"server_dump failed: {dump.stderr}")
    records = [json.loads(line) for line in dump.stdout.splitlines() if line]
    del dump
    verdicts = {}
    for kind, (body, pages) in graded_answers.items():
        verdict = query_oracle.check(records, body, pages)
        verdicts[kind] = {"passed": verdict["passed"] if isinstance(verdict, dict) else verdict.passed,
                          "violations": (verdict["violations"] if isinstance(verdict, dict) else verdict.violations)[:3]}
    rows = sum(len(p["rows"]) for body, pages in graded_answers.values() for p in pages)
    per_kind = {k: {"n": len(v), "p50_s": percentile(v, 0.5), "p99_s": percentile(v, 0.99), "max_s": max(v)}
                for k, v in latency.items()}
    gates = {
        "oracle": all(v["passed"] for v in verdicts.values()),
        "query_p99_le_2s": all(v["p99_s"] <= 2 for v in per_kind.values()),
        "exits": sim_exit == 0 and server_exit == 0,
    }
    if args.mode == "segment":
        gates["freshness_p99_le_5s"] = bool(freshness) and percentile(freshness, 0.99) <= 5
    summary = {
        "seed": args.seed, "mode": args.mode, "seconds": args.seconds, "passed": all(gates.values()), "gates": gates,
        "records_batches": len(records), "graded_rows": rows, "oracle": verdicts, "queries": per_kind,
        "freshness_samples": len(freshness), "freshness_p50_s": percentile(freshness, 0.5),
        "freshness_p99_s": percentile(freshness, 0.99), "freshness_max_s": max(freshness) if freshness else None,
        "probe_errors": len(probe_errors), "probe_error_examples": probe_errors[:3],
        "bytes_at_rest": bytes_at_rest, "server_vmhwm_kib": server_hwm,
        "sim_exit": sim_exit, "server_exit": server_exit,
    }
    (root / "history-summary.json").write_text(json.dumps(summary, sort_keys=True) + "\n")
    print(json.dumps(summary, sort_keys=True))
    return 0 if summary["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
