import sys
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common import *
API = WT + '/target/alpha-p15-claude-api/p15_api'
root = WT + '/target/alpha-p15-claude-apidup'
log, conf = setup(root, 3)
os.makedirs(root + '/spool')
with open(root + '/spool/coverage-unknown', 'w') as f: f.write('123' + NL)
c, o, e = run([API, conf], env=shim_env(root, at=1, mode='eio', kind='unlink', match='coverage-unknown'))
print('api exit', c); print(o.strip()); print(e.strip())
c, o, e = run([DUMP, conf])
print('persisted bodies', [l for l in o.splitlines()])
