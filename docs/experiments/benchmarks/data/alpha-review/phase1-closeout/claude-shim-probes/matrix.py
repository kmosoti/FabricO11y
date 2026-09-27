import sys
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common import *
root = WT + '/target/alpha-p15-claude-matrix'
spool = root + '/spool'
base = WT + '/target/alpha-p15-claude-matrix-base'
log, conf = setup(root, 3)
c, o, e = run([NODE, 'collect', conf]); assert c == 0, e
write_log(log, 3, 2)
shutil.rmtree(base, ignore_errors=True); shutil.copytree(spool, base)
ev, c = trace_events(root, conf)
nev = len(ev)
fails = []
rows = []
def restore():
    shutil.rmtree(spool); shutil.copytree(base, spool)
for mode in ('kill_before', 'kill_after'):
    for n in range(1, nev + 1):
        restore()
        c, o, e = run([NODE, 'collect', conf], env=shim_env(root, at=n, mode=mode))
        ic, d, ie = inspect(conf)
        rr = d.get('recovery_required'); ia = d.get('interrupted_append'); b = d.get('batches')
        last, dc, exact, cnt = drain_and_check(conf, 5)
        sidecar = os.path.exists(spool + '/append-in-progress')
        ok = ic == 0 and rr == 'false' and exact and not sidecar and last[0] == 0
        rows.append((mode, n, ev[n-1].split()[2], ev[n-1].split('/')[-1], c, ic, rr, ia, b, exact, cnt, sidecar))
        if not ok: fails.append(rows[-1] + (ie, last))
for r in rows: print(r)
print('KILL_FAILS', len(fails))
for f in fails: print('FAIL', f)
