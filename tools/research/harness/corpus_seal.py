"""Generate a steady workload whose log bodies are real lines, seal it with the stock server, measure the Segments."""
import os, sys, subprocess, time, shutil, json, signal
from pathlib import Path
import research_paths
sys.argv = ["x", str(research_paths.DATA / "corpus-seal")]
exec(open(Path(__file__).with_name("topk.py")).read().split("result = {}")[0].replace('ROOT = Path(sys.argv[1]).resolve(); ROOT.mkdir(parents=True, exist_ok=True)', 'ROOT = Path(sys.argv[1]).resolve(); ROOT.mkdir(parents=True, exist_ok=True)'))
MIB = 1048576
gen = ROOT / "gen-corpus"
if not list(gen.glob("sealed-*.faj")):
    env = dict(os.environ, SEALBENCH_CORPUS=str(research_paths.DATA / "corpus/all.log"))
    subprocess.run(["taskset", "-c", "3", str(SB), "gen", "steady", str(gen), str(64 * MIB), str(MIB)], check=True, env=env, timeout=3600)
files = sorted(gen.glob("sealed-*.faj")); print("journal files", len(files), sum(f.stat().st_size for f in files) / MIB, "MiB", flush=True)
state = ROOT / "states" / "seg-corpus64"; shutil.rmtree(state, ignore_errors=True); (state / "journal").mkdir(parents=True)
for f in files[:64]: os.link(f, state / "journal" / f.name)
# seal on CPUs 2-3 (the budget run owns 0-1)
port = free_port(); conf = state / "seal.conf"
conf.write_text(f"listen=127.0.0.1:{port}\ntls_cert={ROOT}/server.pem\ntls_key={ROOT}/server.key\nstate_dir={state}\nadmin_token_file={ROOT}/admin-token\njournal_bytes=4294967296\njournal_file_bytes=65536\n")
p = subprocess.Popen(["taskset", "-c", "2-3", str(BIN / "stock" / "fabric-server"), "serve", str(conf)], stdout=open(state / "seal.log", "wb"), stderr=subprocess.STDOUT)
t0 = time.monotonic()
while list((state / "journal").glob("sealed-*")) or len(list((state / "segments").glob("seg-*"))) < min(64, len(files)):
    if time.monotonic() - t0 > 1200: raise SystemExit("sealing stalled")
    time.sleep(0.5)
p.send_signal(signal.SIGTERM); p.wait(timeout=120); print("sealed in", round(time.monotonic() - t0, 1), "s", flush=True)
