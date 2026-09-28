import sys
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common import *
root = WT + '/target/alpha-p15-claude-first'
spool = root + '/spool'
fails = 0
for mode in ('kill_before', 'kill_after'):
    for n in range(1, 33):
        log, conf = setup(root, 3)
        c, o, e = run([NODE, 'collect', conf], env=shim_env(root, at=n, mode=mode))
        files = sorted(os.listdir(spool)) if os.path.isdir(spool) else None
        ic, d, ie = inspect(conf)
        last, dc, exact, cnt = drain_and_check(conf, 3)
        ok = exact and last[0] == 0
        fails += not ok
        print(mode, n, 'collect', c, 'files', files, 'inspect', ic, ie[:60], 'drain', last[0], 'exact', exact, last[2][:120])
print('FIRST_OPEN_FAILS', fails)
