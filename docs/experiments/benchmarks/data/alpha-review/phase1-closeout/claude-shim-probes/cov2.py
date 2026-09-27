import sys, time
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common import *
from fab import frames
from cov import conf_with, unknown_gaps

root = WT + '/target/alpha-p15-claude-cov-clear'
log, conf = setup(root, 3)
conf = conf_with(root, log, 8192)
c1, o1, e1 = run([NODE, 'collect', conf])
first = open(root + '/spool/coverage-unknown').read().strip()
time.sleep(0.05)
c1b, o1b, e1b = run([NODE, 'collect', conf])
second = open(root + '/spool/coverage-unknown').read().strip()
print('two failures exits', c1, c1b, 'since kept earliest', first == second, first)
conf = conf_with(root, log, 16777216)
# SIGKILL just before unlinking coverage-unknown, i.e. after the batch carrying the notice committed
c, o, e = run([NODE, 'collect', conf], env=shim_env(root, at=1, mode='kill_before', kind='unlink', match='coverage-unknown'))
print('kill before clear exit', c, 'marker', os.path.exists(root + '/spool/coverage-unknown'))
for i in range(2):
    c, o, e = run([NODE, 'collect', conf]); print('next', c, o.strip(), e.strip())
print('unknown gaps per batch', unknown_gaps(root))
