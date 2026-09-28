import sys
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common import *
root = WT + '/target/alpha-p15-claude-eio'
spool = root + '/spool'
base = WT + '/target/alpha-p15-claude-eio-base'
log, conf = setup(root, 3)
c, o, e = run([NODE, 'collect', conf]); assert c == 0, e
write_log(log, 3, 2)
shutil.rmtree(base, ignore_errors=True); shutil.copytree(spool, base)
ev, c = trace_events(root, conf)
for n in range(1, len(ev) + 1):
    shutil.rmtree(spool); shutil.copytree(base, spool)
    c, o, e = run([NODE, 'collect', conf], env=shim_env(root, at=n, mode='eio'))
    files = sorted(os.listdir(spool))
    ic, d, ie = inspect(conf)
    c2, o2, e2 = run([NODE, 'collect', conf])
    print(n, ev[n-1].split()[2], ev[n-1].split('/')[-1], '| collect', c, e.strip()[:90], '| files', files, '| inspect', ic, 'rr', d.get('recovery_required'), 'batches', d.get('batches'), 'committed', d.get('committed_bytes'), '| reopen', c2, e2.strip()[:70])
