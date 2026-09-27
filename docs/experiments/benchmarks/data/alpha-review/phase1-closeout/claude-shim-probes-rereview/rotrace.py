import sys, time
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common import *
SHIM2 = WT + '/target/alpha-p15-claude-shim/shim2.so'
root = WT + '/target/alpha-p15-claude-rot'
spool = root + '/spool'
base = WT + '/target/alpha-p15-claude-rot-base'
N = 16 * 140
log, conf = setup(root, 0)
with open(log, 'w') as f:
    for i in range(N): f.write('line-%05d-' % i + 'r' * 3900 + NL)
n = 0
while True:
    c, o, e = run([NODE, 'collect', conf]); n += 1
    assert c == 0, e
    if os.path.getsize(spool + '/batches.faj') >= 8 * 1024 * 1024: break
print('collects to reach 8 MiB', n, os.path.getsize(spool + '/batches.faj'), sorted(os.listdir(spool)))
shutil.rmtree(base, ignore_errors=True); shutil.copytree(spool, base)
ev, c = trace_events(root, conf)
print('rotating collect exit', c, sorted(os.listdir(spool)))
for l in ev: print(l.replace(root + '/', ''))
