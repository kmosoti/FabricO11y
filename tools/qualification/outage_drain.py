"""Outage and drain qualification: one real node buffers through a 30-minute
server outage and must drain within 10 minutes after reconnect.

Runs inside a runner-owned target/alpha-* directory. The node is offered the
registered source (2 lines/s of 512-byte bodies, alternating repetitive and
seeded high entropy) and samples host metrics every 15 s throughout. The
server runs for 60 s, is stopped for the outage, then restarted. Drain time
is from the restart until the node's ACK cursor reaches the batch it had
committed at the restart. Correctness is graded by the frozen delivery
oracle over the node spool, the node's delivery lines and server_dump.
"""

import argparse
import base64
import hashlib
import json
import signal
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import delivery_oracle  # noqa: E402
from delivery_faults import KIND, Server, free_port, make_certs  # noqa: E402
from fleet_tier import AdminClient  # noqa: E402
from workload import SEEDS, entropy_body  # noqa: E402

BEFORE = 60
DRAIN_LIMIT = 600
AFTER = 60
CHILDREN = []
PRODUCTION = None


def inspect(ctl, conf):
    for _ in range(40):
        out = subprocess.run([str(ctl), "inspect", str(conf)], capture_output=True, text=True, timeout=30)
        if out.returncode == 0:
            return dict(p.split("=", 1) for p in out.stdout.split())
        time.sleep(0.05)
    raise RuntimeError(f"inspect failed: {out.stderr}")


def main():
    try:
        return trial()
    finally:
        for child in CHILDREN:
            if child.poll() is None:
                child.kill()
                child.wait(timeout=10)
        if PRODUCTION is not None:
            (PRODUCTION.raw_root / "token").unlink(missing_ok=True)
            PRODUCTION.close()


def trial():
    parser = argparse.ArgumentParser()
    parser.add_argument("--seed", type=lambda v: int(v, 0), required=True)
    parser.add_argument("--bin-dir", required=True)
    # Registered trials use 1800 s; a shorter outage is a smoke test only.
    parser.add_argument("--outage-s", type=int, default=1800)
    from production_access import options, validate_options, ProductionAccess
    options(parser)
    args = parser.parse_args()
    validate_options(args)
    if args.seed not in SEEDS:
        raise SystemExit("unregistered seed")
    root = Path.cwd().resolve()
    if (root / ".fabric-alpha-owned").read_text() != "fabric-alpha-runner-v1\n":
        raise SystemExit("outage trial must run inside a runner-owned directory")
    bins = Path(args.bin_dir).resolve(strict=True)
    global PRODUCTION
    if args.production_access:
        PRODUCTION = ProductionAccess(args, root, soak=False)
        bins, server_conf = PRODUCTION.bins, PRODUCTION.config
        port = int(PRODUCTION.origin.rsplit(':', 1)[1])
        shutil.copy2(PRODUCTION.work / 'ca.pem', root / 'ca.pem')
        class ProductionServer:
            @property
            def proc(self): return PRODUCTION.server
            def start(self): PRODUCTION.start_server()
            def stop(self):
                PRODUCTION.stop_server()
                return PRODUCTION.server.returncode
        server, admin = ProductionServer(), PRODUCTION
    else:
        make_certs(root)
        port = free_port()
        admin_token = hashlib.sha256(f"admin:{args.seed}".encode()).hexdigest()
        (root / "admin-token").write_text(admin_token + "\n")
        server_conf = root / "server.conf"
        server_conf.write_text(
            f"listen=127.0.0.1:{port}\ntls_cert={root}/server.pem\ntls_key={root}/server.key\n"
            f"state_dir={root}/server-state\nadmin_token_file={root}/admin-token\n")
        class LegacyServer(Server):
            def start(self):
                # Server's bounded listen loop is preserved; only the explicit
                # legacy CLI mode changes for the historical adapter.
                import socket
                out = open(self.log, 'ab')
                self.proc = subprocess.Popen([str(self.binary), 'serve-legacy', str(self.config)], stdout=out, stderr=out)
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    if self.proc.poll() is not None: raise RuntimeError('legacy server exited')
                    try:
                        socket.create_connection(('127.0.0.1', self.port), timeout=.2).close()
                        return
                    except OSError: time.sleep(.05)
                raise RuntimeError('legacy server did not listen')
        server = LegacyServer(bins / "fabric-server", server_conf, port, root / "server.log")
        server.start()
        CHILDREN.append(server.proc)
        admin = AdminClient(port, root / "ca.pem", admin_token)
    log = root / "source.log"
    log.touch()
    token = admin.call("POST", body={"name": "node0", "logs": [str(log)], "metric_interval_s": 15})["token"]
    (root / "token").write_text(token + "\n")
    (root / "token").chmod(0o600)
    if PRODUCTION is not None:
        PRODUCTION.bridge.poll_ui(True)
    conf = root / "node.conf"
    conf.write_text(f"spool_dir={root}/spool\nmetric_interval_s=15\nspool_bytes=268435456\n"
                    f"server_url=https://127.0.0.1:{port}\nserver_ca={root}/ca.pem\ntoken_file={root}/token\n")
    node_out = root / "node.out"
    node = subprocess.Popen([str(bins / "fabric-node"), "run", str(conf)], stdout=open(node_out, "wb"),
                            stderr=open(root / "node.err", "wb"))
    CHILDREN.append(node)
    stop_writer = threading.Event()
    offered = [0]

    def writer():
        began = time.monotonic()
        with open(log, "ab") as sink:
            tick = 0
            while not stop_writer.is_set():
                time.sleep(max(0.0, began + tick / 2 - time.monotonic()))
                body = "R" * 512 if tick % 2 == 0 else entropy_body(args.seed, 0, tick)
                sink.write((body + "\n").encode())
                sink.flush()
                offered[0] += 1
                tick += 1

    threading.Thread(target=writer, daemon=True).start()
    time.sleep(BEFORE)
    if PRODUCTION is not None:
        PRODUCTION.bridge.poll_ui(False)
    server.stop()
    outage_started = time.monotonic()
    peak_spool = 0
    node_hwm = 0
    while time.monotonic() - outage_started < args.outage_s:
        time.sleep(15)
        if PRODUCTION is not None:
            PRODUCTION.sample_bound()
        report = inspect(bins / "fabricctl", conf)
        peak_spool = max(peak_spool, int(report["committed_bytes"]))
        node_hwm = max(node_hwm, int([l for l in Path(f"/proc/{node.pid}/status").read_text().splitlines()
                                      if l.startswith("VmHWM:")][0].split()[1]))
        if node.poll() is not None:
            raise SystemExit(f"node exited during the outage: {node.returncode}")
    buffered = inspect(bins / "fabricctl", conf)
    target_seq = int(buffered["next_sequence"]) - 1
    backlog_at_restart = target_seq - int(buffered["acked_through"])
    server.start()
    CHILDREN.append(server.proc)
    restarted = time.monotonic()
    if PRODUCTION is not None:
        PRODUCTION.owner = PRODUCTION.bridge.login()
        PRODUCTION.bridge.poll_ui(True)
    drain_s = None
    while time.monotonic() - restarted < DRAIN_LIMIT + 60:
        report = inspect(bins / "fabricctl", conf)
        if int(report["acked_through"]) >= target_seq:
            drain_s = time.monotonic() - restarted
            break
        time.sleep(1)
    time.sleep(AFTER)
    stop_writer.set()
    time.sleep(3)
    node.send_signal(signal.SIGTERM)
    node_exit = node.wait(timeout=60)
    if PRODUCTION is not None:
        PRODUCTION.bridge.poll_ui(False)
    server_exit = server.stop()

    records = [json.loads(l) for l in subprocess.run([str(bins / "examples" / "spool_dump"), str(conf)],
                                                      capture_output=True, text=True, timeout=300,
                                                      check=True).stdout.splitlines() if l]
    sources = {r["sequence"]: r for r in records if r["type"] == "source"}
    state = [r for r in records if r["type"] == "node_state"]
    transcript = list(sources.values())
    for line in node_out.read_text().splitlines():
        if not line.startswith("delivery "):
            continue
        d = dict(part.split("=", 1) for part in line.split()[1:])
        seq = int(d["sequence"])
        src = sources.get(seq)
        ok = src and hashlib.sha256(base64.b64decode(src["bytes"])).hexdigest() == d["sha256"]
        ident = {"node_id": state[0]["node_id"], "generation": state[0]["generation"], "sequence": seq}
        transcript.append({"type": "attempt", **ident, "injected_conflict": False,
                           "bytes": src["bytes"] if ok else base64.b64encode(d["sha256"].encode()).decode()})
        response = {"type": "response", **ident, "kind": KIND[d["status"]]}
        if d["status"] == "ack":
            response["committed_through"] = int(d["committed_through"])
        transcript.append(response)
    transcript.append({"type": "fault", "label": f"server outage {args.outage_s}s"})
    transcript.extend(state)
    dump = subprocess.run([str(bins / "examples" / "server_dump"), str(server_conf)],
                          capture_output=True, text=True, timeout=300, check=True)
    recovered = [json.loads(line) for line in dump.stdout.splitlines() if line]
    companion = None
    if PRODUCTION is not None:
        import soak_companion
        group = PRODUCTION.server_group
        resources = {name: (group / name).read_text().strip() for name in
                     ('memory.peak', 'memory.events', 'memory.swap.current', 'cgroup.events', 'cpu.stat', 'io.stat', 'pids.peak')}
        events = dict(line.split() for line in resources['memory.events'].splitlines())
        stopped = ('populated 0' in resources['cgroup.events'] and resources['memory.swap.current'] == '0'
                   and int(events.get('oom', 0)) == 0 and int(events.get('oom_kill', 0)) == 0)
        observation = soak_companion.dump_stopped_spool(bins / 'examples/spool_dump',
            PRODUCTION.state_root / 'self-spindle/node.conf', root / 'companion-custody', processes_stopped=stopped)
        companion_records = [soak_companion.project(record) for record in recovered
                             if (record['node_id'], record['generation']) == tuple(observation['stream'])]
        companion = soak_companion.validate_recovery(observation, companion_records)
        companion['resources'] = resources
        transcript.extend(observation['projected_sources'])
        recovered = [soak_companion.project(record) if (record['node_id'], record['generation']) == tuple(observation['stream'])
                     else record for record in recovered]
    transcript.extend(recovered)
    transcript.append({"type": "end"})
    (root / "transcript.jsonl").write_text("\n".join(json.dumps(r) for r in transcript) + "\n")
    verdict = delivery_oracle.check((root / "transcript.jsonl").read_text().splitlines())
    final = inspect(bins / "fabricctl", conf)
    gates = {
        "oracle": verdict.passed and (companion is None or companion["passed"]),
        "drain_le_600s": drain_s is not None and drain_s <= DRAIN_LIMIT,
        "exits": node_exit == 0 and server_exit == 0,
        "all_lines_committed": int(final["log_records"]) >= offered[0],
        "node_rss_le_64mib": node_hwm <= 64 * 1024,
    }
    summary = {"access_mode": "production-passkey-scoped-workload" if args.production_access else "explicit-legacy-admin",
               "companion": companion, "seed": args.seed, "outage_s": args.outage_s, "passed": all(gates.values()), "gates": gates,
               "violations": [v.__dict__ for v in verdict.violations][:5],
               "backlog_batches_at_restart": backlog_at_restart, "drain_s": drain_s,
               "peak_spool_bytes": peak_spool, "node_vmhwm_kib": node_hwm, "offered_lines": offered[0],
               "committed_lines": int(final["log_records"]), "node_exit": node_exit, "server_exit": server_exit}
    (root / "outage-summary.json").write_text(json.dumps(summary, sort_keys=True) + "\n")
    if PRODUCTION is not None:
        PRODUCTION.receipt["passed"] = summary["passed"]
    print(json.dumps(summary, sort_keys=True))
    return 0 if summary["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
