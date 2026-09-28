"""Run real fabric-server and fabric-node processes under injected faults and
grade the result with the frozen delivery oracle.

Scenarios: `clean`, `server-kill` (SIGKILL the server three times and restart
it), `node-kill` (SIGKILL each node at random points and restart it), and
`outage` (server down for 10 s while nodes keep collecting). The transcript
is assembled only from observations the oracle's adapter contract allows:

- `source` and `node_state` records come from `alpha_spool_dump`, run while
  the node process is stopped (after a kill, and at the end);
- `attempt` and `response` records come from the node's own delivery lines,
  which carry the SHA-256 of the exact bytes sent; an attempt whose hash does
  not match its source is emitted with different bytes, so EXACT-RETRY fires;
- `recovered` records come from `server_dump`, a fresh process reading the
  server's durable state after the server has stopped.

Each node process run is one segment: sources first seen at the observation
that ends a segment are emitted before that segment's attempts. That is the
real order, because a node sends only batches it has already committed.

`--mutate drop-recovered` deletes one recovered record before grading; the
oracle must then fail, which shows the pipeline can fail end to end.
Disposable data lives under target/alpha-delivery-<scenario>-<seed>/ and is
removed after a pass unless --keep is given.
"""

import argparse
import base64
import hashlib
import json
import os
import random
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import delivery_oracle  # noqa: E402

KIND = {
    "ack": "ack",
    "conflict": "conflict",
    "gap": "gap",
    "unauthorized": "unauthorized",
    "forbidden": "unauthorized",
    "bad_request": "bad_request",
    "too_large": "too_large",
    "unavailable": "unavailable",
    "no_response": "no_response",
}


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def openssl(root: Path, *args: str) -> None:
    subprocess.run(["openssl", *args], cwd=root, check=True, capture_output=True, timeout=60)


def make_certs(root: Path) -> None:
    openssl(root, "req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256",
            "-nodes", "-keyout", "ca.key", "-out", "ca.pem", "-days", "2", "-subj", "/CN=fabric test CA",
            "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign,cRLSign")
    openssl(root, "req", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes",
            "-keyout", "server.key", "-out", "server.csr", "-subj", "/CN=127.0.0.1")
    (root / "san.ext").write_text("subjectAltName=IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\n"
                                  "keyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n")
    openssl(root, "x509", "-req", "-in", "server.csr", "-CA", "ca.pem", "-CAkey", "ca.key",
            "-CAcreateserial", "-out", "server.pem", "-days", "2", "-extfile", "san.ext")


class Server:
    def __init__(self, binary: Path, config: Path, port: int, log: Path):
        self.binary, self.config, self.port, self.log = binary, config, port, log
        self.proc = None

    def start(self) -> None:
        out = open(self.log, "ab")
        self.proc = subprocess.Popen([str(self.binary), "serve", str(self.config)],
                                     stdout=out, stderr=out)
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if self.proc.poll() is not None:
                raise RuntimeError(f"server exited {self.proc.returncode}; see {self.log}")
            try:
                socket.create_connection(("127.0.0.1", self.port), timeout=0.2).close()
                return
            except OSError:
                time.sleep(0.05)
        raise RuntimeError("server did not listen")

    def kill(self) -> None:
        self.proc.send_signal(signal.SIGKILL)
        self.proc.wait(timeout=10)

    def stop(self) -> int:
        self.proc.send_signal(signal.SIGTERM)
        return self.proc.wait(timeout=30)


class NodeProc:
    def __init__(self, index: int, root: Path, binary: Path, config: Path):
        self.index, self.root, self.binary, self.config = index, root, binary, config
        self.proc = None
        self.runs = 0
        self.segments = []  # (list of delivery dicts, observation lines)

    def start(self) -> None:
        self.runs += 1
        self.out = self.root / f"node{self.index}-run{self.runs}.out"
        self.proc = subprocess.Popen([str(self.binary), "run", str(self.config)],
                                     stdout=open(self.out, "wb"),
                                     stderr=open(self.root / f"node{self.index}-run{self.runs}.err", "wb"))

    def halt(self, sig: int) -> int:
        self.proc.send_signal(sig)
        return self.proc.wait(timeout=30)

    def deliveries(self) -> list:
        found = []
        for line in self.out.read_text().splitlines():
            if not line.startswith("delivery "):
                continue
            fields = dict(part.split("=", 1) for part in line.split()[1:])
            found.append(fields)
        return found


def observe(dump: Path, config: Path) -> list:
    result = subprocess.run([str(dump), str(config)], capture_output=True, text=True, timeout=60)
    if result.returncode != 0:
        raise RuntimeError(f"spool dump failed: {result.stderr}")
    return [json.loads(line) for line in result.stdout.splitlines() if line]


def writer(path: Path, stop: threading.Event, rate: float, index: int) -> None:
    n = 0
    with open(path, "a") as out:
        while not stop.is_set():
            out.write(f"node{index} line {n:06d} " + "x" * 64 + "\n")
            out.flush()
            n += 1
            time.sleep(1 / rate)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--scenario", choices=["clean", "server-kill", "node-kill", "outage"], required=True)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--nodes", type=int, default=3)
    parser.add_argument("--seconds", type=float, default=20)
    parser.add_argument("--bin-dir", default="target/release")
    parser.add_argument("--mutate", choices=["drop-recovered"])
    parser.add_argument("--keep", action="store_true")
    args = parser.parse_args()
    bins = Path(args.bin_dir).resolve()
    node_bin, server_bin = bins / "fabric-node", bins / "fabric-server"
    spool_dump, server_dump = bins / "examples" / "alpha_spool_dump", bins / "examples" / "server_dump"
    root = Path(f"target/alpha-delivery-{args.scenario}-{args.seed}").resolve()
    if root.is_symlink():
        raise SystemExit("refusing symlinked root")
    shutil.rmtree(root, ignore_errors=True)
    root.mkdir(parents=True)
    rng = random.Random(args.seed)
    make_certs(root)
    port = free_port()
    (root / "admin-token").write_text(f"{rng.getrandbits(256):064x}\n")
    server_conf = root / "server.conf"
    server_conf.write_text(
        f"listen=127.0.0.1:{port}\ntls_cert={root}/server.pem\ntls_key={root}/server.key\n"
        f"state_dir={root}/server-state\nadmin_token_file={root}/admin-token\njournal_bytes=268435456\n")
    server = Server(server_bin, server_conf, port, root / "server.log")
    server.start()
    admin_conf = root / "admin.conf"
    admin_conf.write_text(f"server_url=https://127.0.0.1:{port}\nserver_ca={root}/ca.pem\n"
                          f"admin_token_file={root}/admin-token\n")
    for i in range(args.nodes):
        log = root / f"app{i}.log"
        out = subprocess.run([str(bins / "fabricctl"), "admin", str(admin_conf), "node", "add", f"node{i}",
                              "--log", str(log), "--interval", "1"],
                             capture_output=True, text=True, timeout=30)
        if out.returncode != 0:
            raise RuntimeError(f"enroll node{i}: {out.stdout} {out.stderr}")
        (root / f"token{i}").write_text(json.loads(out.stdout)["token"] + "\n")
    nodes, stops, threads, transcript = [], [], [], []
    for i in range(args.nodes):
        conf = root / f"node{i}.conf"
        log = root / f"app{i}.log"
        log.touch()
        conf.write_text(
            f"spool_dir={root}/spool{i}\nlog={log}\nmetric_interval_s=1\nspool_bytes=16777216\n"
            f"server_url=https://127.0.0.1:{port}\nserver_ca={root}/ca.pem\ntoken_file={root}/token{i}\n")
        node = NodeProc(i, root, node_bin, conf)
        node.start()
        nodes.append(node)
        stop = threading.Event()
        stops.append(stop)
        thread = threading.Thread(target=writer, args=(log, stop, 10.0, i), daemon=True)
        thread.start()
        threads.append(thread)

    seen_sources = {}  # node index -> set of sequences already emitted

    def close_segment(node: NodeProc, label: str) -> None:
        records = observe(spool_dump, node.config)
        emitted = seen_sources.setdefault(node.index, set())
        sources = [r for r in records if r["type"] == "source"]
        state = [r for r in records if r["type"] == "node_state"]
        by_seq = {}
        for r in sources:
            by_seq[r["sequence"]] = r
            if r["sequence"] not in emitted:
                transcript.append(r)
                emitted.add(r["sequence"])
        for d in node.deliveries():
            seq = int(d["sequence"])
            src = by_seq.get(seq)
            if src and hashlib.sha256(base64.b64decode(src["bytes"])).hexdigest() == d["sha256"]:
                sent = src["bytes"]
            else:
                sent = base64.b64encode(b"sent-bytes-with-sha256:" + d["sha256"].encode()).decode()
            ident = {"node_id": state[0]["node_id"], "generation": state[0]["generation"], "sequence": seq}
            transcript.append({"type": "attempt", **ident, "bytes": sent, "injected_conflict": False})
            response = {"type": "response", **ident, "kind": KIND[d["status"]]}
            if d["status"] == "ack":
                response["committed_through"] = int(d["committed_through"])
            transcript.append(response)
        transcript.append({"type": "fault", "label": f"node{node.index} {label}"})
        transcript.extend(state)

    started = time.monotonic()
    events = []
    if args.scenario == "server-kill":
        events = sorted(rng.uniform(2, args.seconds - 4) for _ in range(3))
    elif args.scenario == "node-kill":
        events = sorted((rng.uniform(2, args.seconds - 2), rng.randrange(args.nodes)) for _ in range(2 * args.nodes))
    elif args.scenario == "outage":
        events = [3.0]
    for event in events:
        at = event if not isinstance(event, tuple) else event[0]
        time.sleep(max(0.0, started + at - time.monotonic()))
        if args.scenario == "server-kill":
            server.kill()
            transcript.append({"type": "fault", "label": f"server_kill at {at:.2f}s"})
            time.sleep(rng.uniform(0.5, 2.0))
            server.start()
        elif args.scenario == "node-kill":
            node = nodes[event[1]]
            node.halt(signal.SIGKILL)
            close_segment(node, f"node_kill at {at:.2f}s")
            node.start()
        elif args.scenario == "outage":
            server.kill()
            transcript.append({"type": "fault", "label": "server outage 10s"})
            time.sleep(10)
            server.start()
    time.sleep(max(0.0, started + args.seconds - time.monotonic()))
    # Let nodes drain what they can, then stop them gracefully.
    for stop in stops:
        stop.set()
    time.sleep(3)
    exits = [node.halt(signal.SIGTERM) for node in nodes]
    for node in nodes:
        close_segment(node, "final stop")
    server_exit = server.stop()
    dump = subprocess.run([str(server_dump), str(server_conf)], capture_output=True, text=True, timeout=120)
    if dump.returncode != 0:
        print("server_dump failed:", dump.stderr)
        return 1
    recovered = [json.loads(line) for line in dump.stdout.splitlines() if line]
    if args.mutate == "drop-recovered" and recovered:
        acked = [r for r in recovered]
        recovered.remove(acked[len(acked) // 2])
    transcript.extend(recovered)
    transcript.append({"type": "end"})
    (root / "transcript.jsonl").write_text("\n".join(json.dumps(r) for r in transcript) + "\n")
    verdict = delivery_oracle.check((root / "transcript.jsonl").read_text().splitlines())
    counts = {
        "sources": sum(r["type"] == "source" for r in transcript),
        "attempts": sum(r["type"] == "attempt" for r in transcript),
        "acks": sum(r.get("kind") == "ack" for r in transcript),
        "recovered": len(recovered),
        "node_exits": exits,
        "server_exit": server_exit,
    }
    passed = verdict.passed and all(code == 0 for code in exits) and server_exit == 0
    summary = {"scenario": args.scenario, "seed": args.seed, "nodes": args.nodes,
               "passed": passed, "oracle_passed": verdict.passed,
               "violations": [v.__dict__ if hasattr(v, "__dict__") else v for v in verdict.violations][:10],
               **counts}
    print(json.dumps(summary, default=str))
    (root / "summary.json").write_text(json.dumps(summary, default=str) + "\n")
    if passed and not args.keep:
        shutil.rmtree(root)
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
