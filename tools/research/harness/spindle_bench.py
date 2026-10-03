"""Spindle throughput on this machine: the release fabric-node draining a pre-filled log
of real text (the research corpus repeated), pinned to CPU 3, in three modes:
collect only (no server), collect and deliver to the release server (CPUs 0-2,
seal_workers=2), and the same under max_output_bytes_per_s. Reads the node's cycle
lines for consumed log bytes and lines over time, and /proc for its CPU and memory.
Usage: spindle_bench.py <out-root> [seconds] [cap_mib_per_s] [modes: collect-only,deliver,cap]
"""
import json, os, re, shutil, signal, socket, subprocess, sys, threading, time
from pathlib import Path
import research_paths
from delivery_faults import free_port, make_certs

ROOT = Path(sys.argv[1]).resolve(); shutil.rmtree(ROOT, ignore_errors=True); ROOT.mkdir(parents=True)
SECONDS = int(sys.argv[2]) if len(sys.argv) > 2 else 60
CAP = int(sys.argv[3]) if len(sys.argv) > 3 else 8
REL = research_paths.REPO / "target" / "release"
NODE, SERVER, LOAD = REL / "fabric-node", REL / "fabric-server", REL / "examples" / "ingest_load"
MIB = 1 << 20
TICK = os.sysconf("SC_CLK_TCK")
make_certs(ROOT); (ROOT / "admin-token").write_text("ab" * 32 + "\n")
corpus = (research_paths.CORPUS / "all.log").read_bytes()
corpus_lines = corpus.count(b"\n")

def fill(path, mib):
    with open(path, "wb") as f:
        for _ in range(mib * MIB // len(corpus) + 1): f.write(corpus)
    return path.stat().st_size

def proc(pid):
    stat = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
    hwm = next(int(l.split()[1]) // 1024 for l in open(f"/proc/{pid}/status") if l.startswith("VmHWM"))
    return (int(stat[11]) + int(stat[12])) / TICK, hwm

def run_node(name, log_mib, server=None, cap=None, token=0):
    work = ROOT / name; work.mkdir(exist_ok=True)
    log = ROOT / "app.log"; size = fill(log, log_mib)
    conf = work / "node.conf"
    text = f"spool_dir={work}/spool\nlog={log}\nmetric_interval_s=3600\nspool_bytes={256 * MIB}\n"
    if server: text += f"server_url=https://127.0.0.1:{server}\nserver_ca={ROOT}/ca.pem\ntoken_file={ROOT}/tokens/token-{token:04}\n"
    if cap: text += f"max_output_bytes_per_s={cap * MIB}\n"
    conf.write_text(text)
    t0 = time.monotonic()
    node = subprocess.Popen(["taskset", "-c", "3", str(NODE), "run", str(conf)], stdout=subprocess.PIPE, stderr=open(work / "node.err", "w"), text=True)
    points = []; acked = [0]; batch_seq = [0]
    # Every stdout line with its arrival time, for the anatomy of one Batch.
    out = open(work / "node.out", "w")
    def reader():
        for line in node.stdout:
            out.write(f"{time.monotonic() - t0:.6f} {line}"); out.flush()
            m = re.match(r"batch=(\d+) .*logs=(\d+) .*log_backlog_bytes=(\d+) acked_through=(\d+)", line)
            if m:
                batch_seq[0] = int(m[1])
                points.append((time.monotonic() - t0, int(m[2]), int(m[3])))
            m = re.search(r"acked[_ ]through[= ](\d+)", line)
            if m: acked[0] = max(acked[0], int(m[1]))
    th = threading.Thread(target=reader, daemon=True); th.start()
    while time.monotonic() - t0 < SECONDS and node.poll() is None:
        time.sleep(0.5)
        if points and points[-1][2] == 0 and (server is None or acked[0] >= batch_seq[0]): break
    elapsed = time.monotonic() - t0
    try:
        cpu, hwm = proc(node.pid)
    except FileNotFoundError:
        cpu, hwm = float("nan"), 0  # the node exited early; see node.err
    node.send_signal(signal.SIGTERM); node.wait(timeout=30)
    lines = sum(p[1] for p in points); backlog = points[-1][2] if points else size
    consumed = size - backlog
    # The rate over the time collection was busy: until the backlog was all but
    # gone (an unterminated final line may remain), or the whole run.
    busy = next((t for t, _, b in points if b < 4096), elapsed) if points else elapsed
    rec = {"mode": name, "log_bytes": size, "seconds": round(elapsed, 2), "busy_s": round(busy, 2), "consumed_bytes": consumed, "lines": lines,
           "mb_per_s": round(consumed / busy / 1e6, 2), "lines_per_s": round(lines / busy), "node_cpu_s": round(cpu, 2),
           "node_cpu_share": round(cpu / elapsed, 2), "node_hwm_mib": hwm, "batches": len(points), "acked_through": acked[0], "drained": backlog == 0}
    print(json.dumps(rec), flush=True)
    shutil.rmtree(work / "spool", ignore_errors=True); log.unlink()
    return rec

MODES = sys.argv[4].split(",") if len(sys.argv) > 4 else ["collect-only", "deliver", "cap"]
results = [run_node("collect-only", 64)] if "collect-only" in MODES else []
state = ROOT / "server-state"; state.mkdir()
# The server's central configuration replaces a node's log paths, so the enrolled
# configuration names the benchmark's log.
# One enrolled credential per run: a credential binds to the first node identity
# that uses it, and each run has its own Spool and so its own identity.
subprocess.run([str(LOAD), "enroll", str(state), str(ROOT / "tokens"), "2", str(ROOT / "app.log")], check=True)
port = free_port()
(ROOT / "server.conf").write_text(f"listen=127.0.0.1:{port}\ntls_cert={ROOT}/server.pem\ntls_key={ROOT}/server.key\nstate_dir={state}\nadmin_token_file={ROOT}/admin-token\n"
                                  f"journal_bytes={4 << 30}\njournal_file_bytes={64 * MIB}\nretention_bytes={1 << 30}\nseal_workers=2\n")
srv = subprocess.Popen(["taskset", "-c", "0-2", str(SERVER), "serve", str(ROOT / "server.conf")], stdout=open(ROOT / "server.log", "w"), stderr=subprocess.STDOUT)
t0 = time.monotonic()
while True:
    try: socket.create_connection(("127.0.0.1", port), timeout=0.1).close(); break
    except OSError:
        if time.monotonic() - t0 > 30: raise SystemExit("server did not start")
        time.sleep(0.05)
if "deliver" in MODES: results.append(run_node("deliver", 256, server=port))
if "cap" in MODES: results.append(run_node(f"deliver-cap-{CAP}MiB", 256, server=port, cap=CAP, token=1))
srv.send_signal(signal.SIGTERM); srv.wait(timeout=60)
shutil.rmtree(state, ignore_errors=True)
json.dump({"corpus_lines": corpus_lines, "results": results}, open(ROOT / "spindle_bench.json", "w"), indent=1)
