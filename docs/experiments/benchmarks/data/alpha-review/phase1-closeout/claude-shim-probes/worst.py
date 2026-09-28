import sys
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common import *
from fab import frames
root = WT + '/target/alpha-p15-claude-worst'
shutil.rmtree(root, ignore_errors=True); os.makedirs(root + '/spool')
logs = []
for i in range(16):
    name = root + '/' + ('%02d' % i) + chr(0x4E2D) * 60
    name = name.encode()[:239].decode('utf-8', 'ignore')
    with open(name, 'wb') as f:
        for j in range(20): f.write(b'ok\xff\xfe' + str(j).encode() + b'\n')
    logs.append(name)
conf = root + '/node.conf'
with open(conf, 'w') as f:
    f.write('spool_dir=' + root + '/spool' + NL + ''.join('log=' + p + NL for p in logs) + 'metric_interval_s=15' + NL + 'spool_bytes=16777216' + NL)
with open(root + '/spool/coverage-unknown', 'w') as f: f.write('123' + NL)
env = dict(LD_PRELOAD=SHIM, SHIM_PREFIX=chr(47) + 'proc', SHIM_AT='1', SHIM_MODE='eio', SHIM_KIND='open', SHIM_MATCH='stat')
c, o, e = run([NODE, 'collect', conf], env=env)
print('exit', c, o.strip(), e.strip()[:200])
fr = frames(root + '/spool/batches.faj')
for x in fr:
    g = x['gaps']
    print('seq', x['seq'], 'gaps', len(g), 'max bytes', max(len(s.encode()) for s in g) if g else 0, 'kinds', sorted(set(s.split(':')[0][:40] for s in g)))
c, o, e = run([NODE, 'collect', conf]); print('second', c, o.strip(), e.strip()[:200])
