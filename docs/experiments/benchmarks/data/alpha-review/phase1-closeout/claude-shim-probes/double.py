import sys
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common import *
root = WT + '/target/alpha-p15-claude-double'
spool = root + '/spool'
base = WT + '/target/alpha-p15-claude-double-base'
log, conf = setup(root, 3)
c, o, e = run([NODE, 'collect', conf]); assert c == 0
write_log(log, 3, 2)
shutil.rmtree(base, ignore_errors=True); shutil.copytree(spool, base)
fails = 0; total = 0
for k in range(8, 23):
    for m1 in ('kill_before', 'kill_after'):
        shutil.rmtree(spool); shutil.copytree(base, spool)
        run([NODE, 'collect', conf], env=shim_env(root, at=k, mode=m1))
        mid = WT + '/target/alpha-p15-claude-double-mid'
        shutil.rmtree(mid, ignore_errors=True); shutil.copytree(spool, mid)
        ev, _ = trace_events(root, conf)
        for n in range(1, min(len(ev), 10) + 1):
            for m2 in ('kill_before', 'kill_after'):
                shutil.rmtree(spool); shutil.copytree(mid, spool)
                run([NODE, 'collect', conf], env=shim_env(root, at=n, mode=m2))
                ic, d, ie = inspect(conf)
                last, dc, exact, cnt = drain_and_check(conf, 5)
                ok = ic == 0 and d.get('recovery_required') == 'false' and exact and last[0] == 0 and not os.path.exists(spool + '/append-in-progress')
                total += 1
                if not ok:
                    fails += 1; print('FAIL', k, m1, n, m2, ev[n-1], ic, ie, last, exact, cnt)
print('DOUBLE total', total, 'fails', fails)
