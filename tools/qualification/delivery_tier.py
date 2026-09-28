"""One registered phase-2 delivery trial: ten real nodes and one server.

Runs inside a runner-owned target/alpha-* directory (see runner.py). The
registered protocol is docs/experiments/benchmarks/alpha-phase2-delivery-protocol.md.
Prints one JSON summary line and writes delivery-summary.json.
"""

import argparse
import base64
import hashlib
import json
import os
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import delivery_oracle  # noqa: E402
from delivery_faults import KIND, free_port, make_certs  # noqa: E402
from workload import SEEDS, entropy_body  # noqa: E402

NODES = 10
WARMUP = 15
MEASURED = 120
TOTAL = WARMUP + MEASURED
LOGS_PER_SECOND = 2
DRAIN_LIMIT = 45
TICKS = os.sysconf("SC_CLK_TCK")


def cpu_s(pid: int) -> float:
    fields = Path(f"/proc/{pid}/stat").read_text().rpartition(") ")[2].split()
    return (int(fields[11]) + int(fields[12])) / TICKS


def status_kib(pid: int, key: str) -> int:
    for line in Path(f"/proc/{pid}/status").read_text().splitlines():
        if line.startswith(key + ":"):
            return int(line.split()[1])
    raise RuntimeError(f"missing {key}")


def run_capture(argv, timeout=60):
    result = subprocess.run([str(a) for a in argv], capture_output=True, text=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError(f"{argv[0]} exit {result.returncode}: {result.stderr.strip()}")
    return result.stdout


def inspect(ctl: Path, config: Path) -> dict:
    # An inspect racing an append, rotation or reclaim is retryable.
    for attempt in range(20):
        try:
            return dict(p.split("=", 1) for p in run_capture([ctl, "inspect", config], 10).split())
        except RuntimeError:
            if attempt == 19:
                raise
            time.sleep(0.05)


def count_frames(journal: Path) -> int:
    frames = 0
    for path in sorted(journal.glob("*.faj")):
        data = path.read_bytes()
        at = 0
        while at + 16 <= len(data):
            size = int.from_bytes(data[at + 4:at + 8], "little")
            at += 16 + size + 16
            frames += 1
    return frames


def percentile(values, q):
    if not values:
        return None
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(q * len(ordered)))]


CHILDREN = []


def main() -> int:
    try:
        return trial()
    finally:
        for child in CHILDREN:
            if child.poll() is None:
                child.kill()
                child.wait(timeout=10)


def trial() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--seed", type=lambda v: int(v, 0), required=True)
    parser.add_argument("--mode", choices=["grouped", "individual"], required=True)
    parser.add_argument("--bin-dir", required=True)
    args = parser.parse_args()
    if args.seed not in SEEDS:
        raise SystemExit("unregistered seed")
    root = Path.cwd().resolve()
    if (root / ".fabric-alpha-owned").read_text() != "fabric-alpha-runner-v1\n":
        raise SystemExit("delivery trial must run inside a runner-owned directory")
    bins = Path(args.bin_dir).resolve(strict=True)
    node_bin, ctl_bin = bins / "fabric-node", bins / "fabricctl"
    server_bin = bins / "examples" / "server_mode"
    spool_dump, server_dump = bins / "examples" / "spool_dump", bins / "examples" / "server_dump"
    make_certs(root)
    port = free_port()
    creds = []
    for i in range(NODES):
        token = hashlib.sha256(f"token:{args.seed}:{i}".encode()).hexdigest()
        (root / f"token{i}").write_text(token + "\n")
        creds.append(f"{hashlib.sha256(token.encode()).hexdigest()} node{i}")
    (root / "credentials").write_text("\n".join(creds) + "\n")
    server_conf = root / "server.conf"
    server_conf.write_text(
        f"listen=127.0.0.1:{port}\ntls_cert={root}/server.pem\ntls_key={root}/server.key\n"
        f"state_dir={root}/server-state\nnode_credentials={root}/credentials\njournal_bytes=536870912\n")
    server = subprocess.Popen([str(server_bin), str(server_conf), args.mode],
                              stdout=open(root / "server.log", "wb"), stderr=subprocess.STDOUT)
    CHILDREN.append(server)
    import socket
    deadline = time.monotonic() + 15
    while True:
        try:
            socket.create_connection(("127.0.0.1", port), timeout=0.2).close()
            break
        except OSError:
            if time.monotonic() > deadline or server.poll() is not None:
                raise SystemExit("server did not start")
            time.sleep(0.05)

    configs, logs, procs, lines = [], [], [], []
    for i in range(NODES):
        log = root / f"app{i}.log"
        log.touch()
        conf = root / f"node{i}.conf"
        conf.write_text(
            f"spool_dir={root}/spool{i}\nlog={log}\nmetric_interval_s=15\nspool_bytes=67108864\n"
            f"server_url=https://127.0.0.1:{port}\nserver_ca={root}/ca.pem\ntoken_file={root}/token{i}\n")
        configs.append(conf)
        logs.append(log)
        proc = subprocess.Popen([str(node_bin), "run", str(conf)], stdout=subprocess.PIPE,
                                stderr=open(root / f"node{i}.err", "wb"))
        procs.append(proc)
        CHILDREN.append(proc)
        lines.append([])

    def reader(i: int) -> None:
        with open(root / f"node{i}.out", "wb") as copy:
            for raw in procs[i].stdout:
                lines[i].append((time.monotonic(), raw.decode().strip()))
                copy.write(raw)

    readers = [threading.Thread(target=reader, args=(i,), daemon=True) for i in range(NODES)]
    for thread in readers:
        thread.start()

    began = time.monotonic()
    samples, cpu_start = [], {}
    sinks = [open(log, "ab") for log in logs]
    # Samples sit between the nodes' 15 s cycle boundaries.
    next_sample = began + WARMUP + 2.5
    max_lag_ms = 0.0
    for tick in range(TOTAL * LOGS_PER_SECOND):
        due = began + tick / LOGS_PER_SECOND
        time.sleep(max(0.0, due - time.monotonic()))
        max_lag_ms = max(max_lag_ms, (time.monotonic() - due) * 1000)
        for i, sink in enumerate(sinks):
            body = "R" * 512 if tick % 2 == 0 else entropy_body(args.seed, i, tick)
            sink.write((body + "\n").encode())
            sink.flush()
        if tick == WARMUP * LOGS_PER_SECOND:
            cpu_start = {"server": cpu_s(server.pid), "nodes": sum(cpu_s(p.pid) for p in procs)}
        now = time.monotonic()
        if now >= next_sample:
            backlog = []
            for conf in configs:
                report = inspect(ctl_bin, conf)
                backlog.append(int(report["next_sequence"]) - 1 - int(report["acked_through"]))
            samples.append({"t": round(now - began, 1), "max_backlog": max(backlog)})
            next_sample += 5
        for i, proc in enumerate(procs):
            if proc.poll() is not None:
                raise SystemExit(f"node {i} exited early: {proc.returncode}")
    cpu_end = {"server": cpu_s(server.pid), "nodes": sum(cpu_s(p.pid) for p in procs)}
    measured_end = time.monotonic()
    for sink in sinks:
        sink.close()
    offered = TOTAL * LOGS_PER_SECOND
    drain_deadline = time.monotonic() + DRAIN_LIMIT
    drained = False
    while time.monotonic() < drain_deadline:
        reports = [inspect(ctl_bin, conf) for conf in configs]
        if all(int(r["log_records"]) >= offered and int(r["acked_through"]) == int(r["next_sequence"]) - 1
               for r in reports):
            drained = True
            break
        time.sleep(0.5)
    hwm = {"server_kib": status_kib(server.pid, "VmHWM"),
           "node_max_kib": max(status_kib(p.pid, "VmHWM") for p in procs)}
    for proc in procs:
        proc.send_signal(signal.SIGTERM)
    node_exits = [proc.wait(timeout=30) for proc in procs]
    for thread in readers:
        thread.join(timeout=5)

    transcript = []
    for i, conf in enumerate(configs):
        records = [json.loads(l) for l in run_capture([spool_dump, conf]).splitlines() if l]
        sources = {r["sequence"]: r for r in records if r["type"] == "source"}
        state = [r for r in records if r["type"] == "node_state"]
        transcript.extend(sources.values())
        for _, line in lines[i]:
            if not line.startswith("delivery "):
                continue
            d = dict(part.split("=", 1) for part in line.split()[1:])
            seq = int(d["sequence"])
            src = sources.get(seq)
            ok = src and hashlib.sha256(base64.b64decode(src["bytes"])).hexdigest() == d["sha256"]
            sent = src["bytes"] if ok else base64.b64encode(d["sha256"].encode()).decode()
            ident = {"node_id": state[0]["node_id"], "generation": state[0]["generation"], "sequence": seq}
            transcript.append({"type": "attempt", **ident, "bytes": sent, "injected_conflict": False})
            response = {"type": "response", **ident, "kind": KIND[d["status"]]}
            if d["status"] == "ack":
                response["committed_through"] = int(d["committed_through"])
            transcript.append(response)
        transcript.extend(state)
    server.send_signal(signal.SIGTERM)
    server_exit = server.wait(timeout=30)
    recovered = [json.loads(l) for l in run_capture([server_dump, server_conf], 120).splitlines() if l]
    transcript.extend(recovered)
    transcript.append({"type": "end"})
    (root / "transcript.jsonl").write_text("\n".join(json.dumps(r) for r in transcript) + "\n")
    verdict = delivery_oracle.check((root / "transcript.jsonl").read_text().splitlines())

    latencies_ms = []
    for i in range(NODES):
        for at, line in lines[i]:
            if line.startswith("delivery ") and " status=ack " in line and began + WARMUP <= at <= measured_end:
                d = dict(part.split("=", 1) for part in line.split()[1:])
                latencies_ms.append(int(d["elapsed_us"]) / 1000)
    live_bytes = sum(p.stat().st_size for p in root.rglob("*") if p.is_file())
    first, last = (samples[0]["max_backlog"], samples[-1]["max_backlog"]) if samples else (None, None)
    p99 = percentile(latencies_ms, 0.99)
    passed = (verdict.passed and drained and all(c == 0 for c in node_exits) and server_exit == 0
              and p99 is not None and p99 <= 1000 and last is not None and last <= first + 1
              and hwm["node_max_kib"] <= 64 * 1024)
    summary = {
        "seed": args.seed, "mode": args.mode, "nodes": NODES, "passed": passed,
        "oracle_passed": verdict.passed, "violations": [v.__dict__ for v in verdict.violations][:5],
        "drained": drained, "node_exits": node_exits, "server_exit": server_exit,
        "acked_attempts_measured": len(latencies_ms),
        "ack_ms_p50": percentile(latencies_ms, 0.50), "ack_ms_p99": p99,
        "ack_ms_max": max(latencies_ms) if latencies_ms else None,
        "backlog_first": first, "backlog_last": last,
        "backlog_max": max(s["max_backlog"] for s in samples) if samples else None,
        "server_cpu_s": round(cpu_end["server"] - cpu_start["server"], 3),
        "nodes_cpu_s": round(cpu_end["nodes"] - cpu_start["nodes"], 3),
        **hwm,
        "server_frames": count_frames(root / "server-state" / "journal"),
        "recovered_batches": len(recovered),
        "live_bytes_end": live_bytes, "max_source_lag_ms": round(max_lag_ms, 3),
    }
    (root / "delivery-summary.json").write_text(json.dumps(summary, sort_keys=True) + "\n")
    print(json.dumps(summary, sort_keys=True))
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
