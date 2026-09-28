import sys, time, signal, random
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common import *
root = WT + '/target/alpha-p15-claude-sig'
log, conf = setup(root, 3)
with open(conf) as f: text = f.read()
with open(conf, 'w') as f: f.write(text.replace('metric_interval_s=15', 'metric_interval_s=3600'))
res = []
rng = random.Random(5)
for i, (sig, delay) in enumerate([(signal.SIGTERM, 0.5), (signal.SIGINT, 0.5), (signal.SIGTERM, 0.0), (signal.SIGTERM, 0.002)] + [(signal.SIGTERM, rng.uniform(0, 0.03)) for _ in range(20)]):
    write_log(log, 3 + i, 1)
    p = subprocess.Popen([NODE, 'run', conf], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    time.sleep(delay)
    t0 = time.time(); p.send_signal(sig)
    out, err = p.communicate(timeout=10)
    dt = time.time() - t0
    ic, d, ie = inspect(conf)
    res.append((sig.name, round(delay, 4), p.returncode, round(dt, 3), out.strip()[:60], err.strip()[:60], ic, d.get('interrupted_append')))
for r in res: print(r)
last, dc, exact, cnt = drain_and_check(conf, 3 + len(res))
print('drain', last, 'exact', exact, cnt)
