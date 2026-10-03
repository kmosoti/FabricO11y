"""Ingest throughput on this machine (ADR-0025): the product server with 1, 2 and 3
sealing workers under the ingest_load generator (one Batch in flight per node, real
log text, three spans per Batch). Server on CPUs 0-2, load on CPU 3. Each run lasts
SECONDS; the journal is capped at 1 GiB and Segment retention at 512 MiB so the disk
holds. Samples every second: sealed journal files waiting, Segments, server CPU
seconds and resident memory.
Usage: ingest.py <out-root> [seconds] [nodes] [body_kib] [workers,...]
"""
import json, os, shutil, signal, socket, subprocess, sys, time
from pathlib import Path
import research_paths
from delivery_faults import free_port, make_certs

ROOT = Path(sys.argv[1]).resolve(); ROOT.mkdir(parents=True, exist_ok=True)
SECONDS = int(sys.argv[2]) if len(sys.argv) > 2 else 30
NODES = int(sys.argv[3]) if len(sys.argv) > 3 else 32
BODY = int(sys.argv[4]) if len(sys.argv) > 4 else 512
WORKERS = [int(w) for w in (sys.argv[5] if len(sys.argv) > 5 else "1,2,3").split(",")]
SERVER = research_paths.REPO / "target" / "release" / "fabric-server"
LOAD = research_paths.REPO / "target" / "release" / "examples" / "ingest_load"
CORPUS = research_paths.CORPUS / "all.log"
GIB = 1 << 30
make_certs(ROOT)
(ROOT / "admin-token").write_text("ab" * 32 + "\n")
TICK = os.sysconf("SC_CLK_TCK")

def proc_sample(pid):
    stat = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
    cpu = (int(stat[11]) + int(stat[12])) / TICK
    rss = hwm = 0
    for l in open(f"/proc/{pid}/status"):
        if l.startswith("VmRSS"): rss = int(l.split()[1]) // 1024
        if l.startswith("VmHWM"): hwm = int(l.split()[1]) // 1024
    return cpu, rss, hwm

results = []
for workers in WORKERS:
    state = ROOT / f"state-w{workers}"; shutil.rmtree(state, ignore_errors=True); state.mkdir()
    tokens = ROOT / f"tokens-w{workers}"; shutil.rmtree(tokens, ignore_errors=True)
    subprocess.run([str(LOAD), "enroll", str(state), str(tokens), str(NODES)], check=True)
    port = free_port()
    conf = ROOT / f"server-w{workers}.conf"
    conf.write_text(f"listen=127.0.0.1:{port}\ntls_cert={ROOT}/server.pem\ntls_key={ROOT}/server.key\nstate_dir={state}\nadmin_token_file={ROOT}/admin-token\n"
                    f"journal_bytes={GIB}\njournal_file_bytes={64 << 20}\nretention_bytes={GIB // 2}\nseal_workers={workers}\n")
    log = open(ROOT / f"server-w{workers}.log", "wb")
    srv = subprocess.Popen(["taskset", "-c", "0-2", str(SERVER), "serve", str(conf)], stdout=log, stderr=subprocess.STDOUT)
    t0 = time.monotonic()
    while True:
        try: socket.create_connection(("127.0.0.1", port), timeout=0.1).close(); break
        except OSError:
            if srv.poll() is not None or time.monotonic() - t0 > 30: raise SystemExit("server did not start")
            time.sleep(0.05)
    cpu0, _, _ = proc_sample(srv.pid)
    # stderr to a file: refused sends are logged per attempt, and a pipe read only at
    # the end fills and blocks the generator.
    load_err = open(ROOT / f"load-w{workers}.err", "w")
    load = subprocess.Popen(["taskset", "-c", "3", str(LOAD), "run", f"https://127.0.0.1:{port}", str(ROOT / "ca.pem"), str(tokens), str(NODES), str(SECONDS), str(CORPUS), str(BODY)],
                            stdout=subprocess.PIPE, stderr=load_err, text=True)
    samples = []
    while load.poll() is None:
        time.sleep(1)
        journal = state / "journal"
        waiting = len(list(journal.glob("sealed-*.faj"))) if journal.exists() else 0
        segs = len(list((state / "segments").glob("seg-*"))) if (state / "segments").exists() else 0
        cpu, rss, hwm = proc_sample(srv.pid)
        samples.append({"t": round(time.monotonic() - t0, 1), "sealed_waiting": waiting, "segments": segs, "cpu_s": round(cpu - cpu0, 2), "rss_mib": rss})
    out, _ = load.communicate()
    load_err.close()
    err = open(ROOT / f"load-w{workers}.err").read()
    summary = json.loads(out.strip().splitlines()[-1])
    # Let sealing drain, then stop.
    drain_t = time.monotonic()
    while len(list((state / "journal").glob("sealed-*.faj"))) and time.monotonic() - drain_t < 120: time.sleep(0.5)
    drain_s = round(time.monotonic() - drain_t, 1)
    cpu, rss, hwm = proc_sample(srv.pid)
    srv.send_signal(signal.SIGTERM); srv.wait(timeout=60)
    rec = {"seal_workers": workers, "nodes": NODES, "body_kib": BODY, "seconds": SECONDS, "load": summary,
           "server_cpu_s": round(cpu - cpu0, 1), "server_hwm_mib": hwm, "max_sealed_waiting": max(s["sealed_waiting"] for s in samples),
           "sealing_drain_after_s": drain_s, "load_refusals": len(err.strip().splitlines()), "load_errors": err.strip().splitlines()[-3:], "samples": samples}
    results.append(rec)
    print(f"workers={workers} {summary['mb_per_s']:.1f} MB/s batches={summary['batches']} p50={summary['latency_ms']['p50']:.1f} ms p99={summary['latency_ms']['p99']:.1f} ms "
          f"cpu={rec['server_cpu_s']} s hwm={hwm} MiB max_sealed_waiting={rec['max_sealed_waiting']} drain={drain_s} s errors={len(err.strip().splitlines())}", flush=True)
    shutil.rmtree(state, ignore_errors=True)
json.dump(results, open(ROOT / "ingest.json", "w"), indent=1)
