import sys, time, signal, random
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common import *
from fab import frames
root = WT + '/target/alpha-p15-claude-runkill'
log, conf = setup(root, 0)
rng = random.Random(9)
# 1) quiet polls write nothing
p = subprocess.Popen([NODE, 'run', conf], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
time.sleep(3.5); p.send_signal(signal.SIGTERM); o, e = p.communicate(timeout=10)
print('quiet run exit', p.returncode, 'batches', len(frames(root + '/spool/batches.faj')), 'stdout lines', len(o.splitlines()))
# 2) a partial line is not committed until its newline arrives
with open(log, 'a') as f: f.write('line-00000')
p = subprocess.Popen([NODE, 'run', conf], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
time.sleep(1.6)
with open(log, 'a') as f: f.write(NL)
time.sleep(1.4); p.send_signal(signal.SIGTERM); o, e = p.communicate(timeout=10)
c, dump, _ = run([DUMP, conf])
print('partial line run exit', p.returncode, 'dump', dump.split(), 'stdout', [l[:40] for l in o.splitlines()])
# 3) SIGKILL run mode at random times while lines arrive, including during 1 s log polls
n = 1; kills = 0
t_end = time.time() + 70
while time.time() < t_end:
    p = subprocess.Popen([NODE, 'run', conf], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    life = rng.uniform(0.05, 2.5); t0 = time.time()
    while time.time() - t0 < life:
        with open(log, 'a') as f:
            for _ in range(rng.randint(0, 3)):
                f.write('line-%05d' % n + NL); n += 1
        time.sleep(rng.uniform(0.0, 0.2))
    p.send_signal(signal.SIGKILL); p.communicate(); kills += 1
    ic, d, ie = inspect(conf)
    if ic != 0 or d.get('recovery_required') != 'false':
        print('INSPECT FAIL', ic, d, ie); break
last, dc, exact, cnt = drain_and_check(conf, n)
print('run-mode kills', kills, 'lines written', n, 'drain', last[0], last[2][:80], 'exact', exact, cnt)
