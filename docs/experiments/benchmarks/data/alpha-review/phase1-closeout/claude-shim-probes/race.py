import sys
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common import *
root = WT + '/target/alpha-p15-claude-race'
msgs = dict()
bad = 0
for t in range(150):
    log, conf = setup(root, 2)
    ps = [subprocess.Popen([NODE, 'collect', conf], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) for _ in range(3)]
    for p in ps:
        o, e = p.communicate()
        key = (p.returncode, e.strip()[:80]); msgs[key] = msgs.get(key, 0) + 1
    last, dc, exact, cnt = drain_and_check(conf, 2)
    bad += not (exact and last[0] == 0)
print(msgs); print('race bad', bad)
