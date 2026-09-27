import sys
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common import *
root = WT + '/target/alpha-p15-claude-backlog2'
log, conf = setup(root, 5)
c, o, e = run([NODE, 'collect', conf]); print('first', c, o.strip())
# rewrite in place (same inode, O_TRUNC) with a different, longer content
with open(log, 'w') as f:
    for i in range(8): f.write('other-%03d' % i + NL)
size = os.path.getsize(log)
ic, d, ie = inspect(conf)
c, o, e = run([NODE, 'collect', conf])
print('size', size, 'inspect backlog', d.get('log_backlog_bytes'), '| next collect', o.strip())
