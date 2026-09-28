import sys
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common import *
root = WT + '/target/alpha-p15-claude-settle'
spool = root + '/spool'
base = WT + '/target/alpha-p15-claude-settle-base'
log, conf = setup(root, 3)
c, o, e = run([NODE, 'collect', conf]); assert c == 0
write_log(log, 3, 2)
shutil.rmtree(base, ignore_errors=True); shutil.copytree(spool, base)
bad = 0; total = 0
for k, m1 in ((12, 'kill_after'), (16, 'kill_after'), (17, 'kill_after'), (18, 'kill_after'), (19, 'kill_after'), (21, 'kill_after')):
    shutil.rmtree(spool); shutil.copytree(base, spool)
    run([NODE, 'collect', conf], env=shim_env(root, at=k, mode=m1))
    mid = WT + '/target/alpha-p15-claude-settle-mid'
    shutil.rmtree(mid, ignore_errors=True); shutil.copytree(spool, mid)
    ev, _ = trace_events(root, conf)
    # reopen events up to the unlink of append-in-progress (the settle window)
    upto = next(i for i, l in enumerate(ev) if 'unlink' in l and 'append-in-progress' in l) + 1
    for n in range(1, upto + 1):
        shutil.rmtree(spool); shutil.copytree(mid, spool)
        c, o, e = run([NODE, 'collect', conf], env=shim_env(root, at=n, mode='eio'))
        rr = os.path.exists(spool + '/recovery-required')
        c2, o2, e2 = run([NODE, 'collect', conf])
        kind = ev[n-1].split()[2] + ' ' + ev[n-1].split('/')[-1]
        # A failed write-type call (fsync/unlink/ftruncate/write/rename) while settling must record recovery-required
        wtype = ev[n-1].split()[2] in ('fsync', 'unlink', 'ftruncate', 'write', 'rename')
        ok = (c != 0) and ((rr and c2 != 0) if wtype else True)
        total += 1; bad += not ok
        print(k, m1, n, kind, '| exit', c, 'rr', rr, '| next', c2, e2.strip()[:60], '' if ok else '  <-- FAIL')
print('SETTLE total', total, 'bad', bad)
