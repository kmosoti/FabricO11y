import sys
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common import *
from fab import frames
root = WT + '/target/alpha-p15-claude-starve'
shutil.rmtree(root, ignore_errors=True); os.makedirs(root)
Q = root + '/a-quiet.log'; B = root + '/b-busy.log'
expA = []
with open(B, 'w') as f:
    for i in range(300):
        if i % 10 == 5: f.write('Z' * 5000 + NL)
        else:
            s = 'B%04d-' % i + 'y' * 900; f.write(s + NL); expA.append(s)
open(Q, 'w').close()
conf = root + '/node.conf'
with open(conf, 'w') as f:
    f.write('spool_dir=' + root + '/spool' + NL + 'log=' + Q + NL + 'log=' + B + NL + 'metric_interval_s=15' + NL + 'spool_bytes=16777216' + NL)
first_seen = dict()
for cyc in range(1, 9):
    with open(Q, 'a') as f: f.write('Q%02d' % cyc + NL)
    with open(B, 'a') as f:
        for j in range(70):
            s = 'C%02d-%03d-' % (cyc, j) + 'z' * 900; f.write(s + NL); expA.append(s)
    c, o, e = run([NODE, 'collect', conf])
    print(cyc, o.strip())
c, o, e = run([DUMP, conf])
bodies = o.splitlines()
qs = [b for b in bodies if b.startswith('Q')]
print('quiet lines committed', qs)
fr = frames(root + '/spool/batches.faj')
for x in fr: print('seq', x['seq'], 'cursors', x['cursors'][0][1] if x['cursors'] else None, [c[1] for c in x['cursors']], 'gaps', len(x['gaps']))
for _ in range(20):
    c, o, e = run([NODE, 'collect', conf])
    if ' logs=0 ' in o: break
c, o, e = run([DUMP, conf])
busy = [b for b in o.splitlines() if not b.startswith('Q')]
print('busy exact', busy == expA, len(busy), len(expA))
