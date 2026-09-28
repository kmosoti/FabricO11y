import os, shutil, subprocess, sys, json

WT = '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed'
BIN = WT + '/target/release'
NODE = BIN + '/fabric-node'
CTL = BIN + '/fabricctl'
DUMP = BIN + '/examples/alpha_native_dump'
SHIM = WT + '/target/alpha-p15-claude-shim/shim.so'
NL = chr(10)

def run(cmd, env=None, timeout=30):
    e = dict(os.environ)
    if env: e.update(env)
    p = subprocess.run(cmd, capture_output=True, text=True, env=e, timeout=timeout)
    return p.returncode, p.stdout, p.stderr

def shim_env(root, at=None, mode=None, kind=None, match=None, trace=None):
    env = dict(LD_PRELOAD=SHIM, SHIM_PREFIX=root + '/spool')
    pairs = (('SHIM_AT', at), ('SHIM_MODE', mode), ('SHIM_KIND', kind), ('SHIM_MATCH', match), ('SHIM_TRACE', trace))
    env.update([(k, str(v)) for k, v in pairs if v is not None])
    return env

def write_log(log, start, n):
    with open(log, 'a') as f:
        for i in range(start, start + n): f.write('line-%05d' % i + NL)

def setup(root, nlines, extra=''):
    shutil.rmtree(root, ignore_errors=True); os.makedirs(root)
    log = root + '/app.log'; conf = root + '/node.conf'
    open(log, 'w').close(); write_log(log, 0, nlines)
    with open(conf, 'w') as f:
        f.write('spool_dir=' + root + '/spool' + NL + 'log=' + log + NL + 'metric_interval_s=15' + NL + 'spool_bytes=16777216' + NL + extra)
    return log, conf

def inspect(conf):
    c, o, e = run([CTL, 'inspect', conf])
    d = dict()
    for tok in o.split():
        if '=' in tok:
            k, v = tok.split('=', 1); d[k] = v
    return c, d, e.strip()

def drain_and_check(conf, nlines):
    last = None
    for _ in range(nlines // 128 + 4):
        c, o, e = run([NODE, 'collect', conf])
        last = (c, o.strip(), e.strip())
        if c != 0 or ' logs=0 ' in o: break
    c, o, e = run([DUMP, conf])
    bodies = [l for l in o.splitlines() if l.startswith('line-')]
    exp = ['line-%05d' % n for n in range(nlines)]
    return last, c, bodies == exp, len(bodies)

def trace_events(root, conf):
    c, o, e = run([NODE, 'collect', conf], env=shim_env(root, trace=1))
    return [l for l in e.splitlines() if l.startswith('SHIM ')], c
