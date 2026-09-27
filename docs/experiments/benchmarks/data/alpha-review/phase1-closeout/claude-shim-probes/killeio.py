import sys
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common import *
root = WT + '/target/alpha-p15-claude-killeio'
spool = root + '/spool'
log, conf = setup(root, 3)
c, o, e = run([NODE, 'collect', conf]); assert c == 0, e
write_log(log, 3, 2)
# step 1: SIGKILL right after the last commit-marker write, before its fsync (event 18 of an append cycle)
c, o, e = run([NODE, 'collect', conf], env=shim_env(root, at=18, mode='kill_after'))
print('step1 kill exit', c, sorted(os.listdir(spool)))
# step 2: restart; the reopen fsync of batches.faj (first sync covering the marker) reports EIO
c, o, e = run([NODE, 'collect', conf], env=shim_env(root, at=1, mode='eio', kind='fsync', match='batches.faj', trace=1))
print('step2 reopen exit', c, e.strip().splitlines()[-1], sorted(os.listdir(spool)))
# step 3: restart again with no fault
ic, d, ie = inspect(conf)
print('step3 inspect', ic, 'rr', d.get('recovery_required'), 'interrupted', d.get('interrupted_append'), 'batches', d.get('batches'), 'logs', d.get('log_records'))
c, o, e = run([NODE, 'collect', conf])
print('step3 reopen exit', c, o.strip(), e.strip(), sorted(os.listdir(spool)))
c, o, e = run([DUMP, conf])
print('dump', [l for l in o.splitlines() if l.startswith('line-')])
