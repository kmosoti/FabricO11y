import sys
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common import *
from fab import frames
root = WT + '/target/alpha-p15-claude-backlog'
shutil.rmtree(root, ignore_errors=True); os.makedirs(root)
A = root + '/a-busy.log'; B = root + '/b-quiet.log'; C = root + '/c-quiet.log'
with open(A, 'w') as f:
    for i in range(400): f.write('A%04d-' % i + 'x' * 1000 + NL)
with open(B, 'w') as f:
    for i in range(5): f.write('B%04d' % i + NL)
with open(C, 'w') as f:
    for i in range(5): f.write('C%04d' % i + NL)
conf = root + '/node.conf'
with open(conf, 'w') as f:
    f.write('spool_dir=' + root + '/spool' + NL + 'log=' + A + NL + 'log=' + B + NL + 'log=' + C + NL + 'metric_interval_s=15' + NL + 'spool_bytes=16777216' + NL)
def truth():
    fr = frames(root + '/spool/batches.faj')
    cur = dict()
    for x in fr:
        for p, off in x['cursors']: cur[p] = off
    return sum(os.path.getsize(p) - cur.get(p, 0) for p in (A, B, C)), cur
for i in range(4):
    c, o, e = run([NODE, 'collect', conf])
    t, cur = truth()
    ic, d, ie = inspect(conf)
    print('cycle', i + 1, o.strip(), '| truth', t, '| inspect backlog', d.get('log_backlog_bytes'), '| B/C offsets', cur.get(B), cur.get(C))
# truncate-and-rewrite B shorter than its cursor: node will re-read all of it
with open(B, 'w') as f: f.write('NEW-B-LINE-1' + NL + 'NEW-B-LINE-2' + NL)
ic, d, ie = inspect(conf)
print('after truncating B to', os.path.getsize(B), 'bytes: inspect backlog', d.get('log_backlog_bytes'), 'A backlog', os.path.getsize(A) - truth()[1].get(A, 0))
c, o, e = run([NODE, 'collect', conf]); print('collect', o.strip())
fr = frames(root + '/spool/batches.faj'); print('gaps in last batch', fr[-1]['gaps'])
