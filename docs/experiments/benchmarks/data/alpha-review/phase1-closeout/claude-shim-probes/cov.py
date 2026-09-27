import sys
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common import *
from fab import frames

def conf_with(root, log, cap):
    conf = root + '/node.conf'
    with open(conf, 'w') as f:
        f.write('spool_dir=' + root + '/spool' + NL + 'log=' + log + NL + 'metric_interval_s=15' + NL + 'spool_bytes=' + str(cap) + NL)
    return conf

def unknown_gaps(root):
    fr = frames(root + '/spool/batches.faj')
    return [(x['seq'], [g for g in x['gaps'] if 'coverage unknown' in g]) for x in fr]

def case(name, fault_env):
    root = WT + '/target/alpha-p15-claude-cov-' + name
    log, conf = setup(root, 3)
    conf = conf_with(root, log, 8192)
    c1, o1, e1 = run([NODE, 'collect', conf], env=fault_env(root))
    marker = root + '/spool/coverage-unknown'
    mtext = open(marker).read() if os.path.exists(marker) else None
    conf = conf_with(root, log, 16777216)
    results = []
    for i in range(3):
        c, o, e = run([NODE, 'collect', conf])
        results.append((c, o.strip(), e.strip()))
    print('CASE', name)
    print('  fail-cycle exit', c1, repr(e1.strip()[:160]), 'marker', repr(mtext))
    for r in results: print('  next', r)
    print('  marker after', os.path.exists(marker))
    try: print('  unknown gaps per batch', unknown_gaps(root))
    except Exception as ex: print('  decode err', ex)
    ic, d, ie = inspect(conf); print('  inspect', ic, 'coverage_unknown', d.get('coverage_unknown'), ie)

case('plain', lambda r: dict())
case('killmarker', lambda r: shim_env(r, at=1, mode='kill_after', kind='create', match='coverage-unknown'))
case('eiomarker', lambda r: shim_env(r, at=1, mode='eio', kind='write', match='coverage-unknown'))
