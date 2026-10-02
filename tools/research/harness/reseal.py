"""Seal a directory of journal files into Segments with a given server binary:
hard-link every sealed-*.faj into a fresh state's journal, start the server and wait
until no sealed file is left. Usage: reseal.py <binary-tag> <journal-src> <out-state>
"""
import os, shutil, sys, time
from pathlib import Path
import research_paths
sys.argv, args = [sys.argv[0], str(Path(sys.argv[3]).parent / "work")], sys.argv[1:]
exec(open(Path(__file__).with_name("l21.py")).read().split("if research_paths.SMOKE:")[0])
which, src, out = args[0], Path(args[1]), Path(args[2])
shutil.rmtree(out, ignore_errors=True); (out / "journal").mkdir(parents=True)
files = sorted(src.glob("sealed-*.faj"))
for f in files: os.link(f, out / "journal" / f.name)
p, q, _ = server(which, out)
t0 = time.monotonic()
while list((out / "journal").glob("sealed-*.faj")):
    if time.monotonic() - t0 > 1800: raise SystemExit("sealing did not finish")
    time.sleep(0.5)
stop(p)
print(out, len(files), "files sealed into", len(list((out / "segments").glob("seg-*"))), "segments in", round(time.monotonic() - t0, 1), "s")
