import sys
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common import *
from fab import frames

def conf_with(root, log, cap):
    conf = root + '/node.conf'
    with open(conf, 'w') as f:
        f.write('spool_dir=' + root + '/spool' + NL + 'log=' + log + NL + 'metric_interval_s=15' + NL + 'spool_bytes=' + str(cap) + NL)
    return conf

def gaps(root):
    return [(x['seq'], [g for g in x['gaps'] if 'coverage unknown' in g]) for x in frames(root + '/spool/batches.faj')]

def seeded(name, content):
    root = WT + '/target/alpha-p15-claude-cov3-' + name
    log, conf = setup(root, 3); os.makedirs(root + '/spool')
    with open(root + '/spool/coverage-unknown', 'wb') as f: f.write(content)
    r = [run([NODE, 'collect', conf])[0] for _ in range(2)]
    print('SEEDED', name, 'exits', r, 'gaps', gaps(root), 'marker left', os.path.exists(root + '/spool/coverage-unknown'))

seeded('empty', b'')
seeded('oldtext', b'collection could not commit; coverage unknown' + NL.encode())

def faulted(name, env_fn):
    root = WT + '/target/alpha-p15-claude-cov3-' + name
    log, conf = setup(root, 3)
    conf = conf_with(root, log, 8192)
    c1, o, e = run([NODE, 'collect', conf], env=env_fn(root))
    files = sorted(os.listdir(root + '/spool'))
    conf = conf_with(root, log, 16777216)
    r = [run([NODE, 'collect', conf])[0] for _ in range(2)]
    print('FAULT', name, 'fail exit', c1, 'files', files, 'next exits', r, 'gaps', gaps(root))

faulted('kill_after_tmp_write', lambda r: shim_env(r, at=1, mode='kill_after', kind='write', match='coverage-unknown.tmp'))
faulted('kill_before_rename', lambda r: shim_env(r, at=1, mode='kill_before', kind='rename', match='coverage-unknown'))
faulted('kill_after_rename', lambda r: shim_env(r, at=1, mode='kill_after', kind='rename', match='coverage-unknown'))
faulted('eio_tmp_fsync', lambda r: shim_env(r, at=1, mode='eio', kind='fsync', match='coverage-unknown.tmp'))
faulted('eio_rename', lambda r: shim_env(r, at=1, mode='eio', kind='rename', match='coverage-unknown'))
