import sys, time, signal
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common import *
root = WT + '/target/alpha-p15-claude-locks'
log, conf = setup(root, 3)
with open(conf) as f: text = f.read()
with open(conf, 'w') as f: f.write(text.replace('metric_interval_s=15', 'metric_interval_s=1'))
out = []
for trial in range(5):
    a = subprocess.Popen([NODE, 'run', conf], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    time.sleep(0.3)
    b = [run([NODE, 'collect', conf]) for _ in range(5)]
    ic, d, ie = inspect(conf)
    write_log(log, 3 + trial, 1)
    time.sleep(1.2)
    a.send_signal(signal.SIGTERM); ao, ae = a.communicate(timeout=10)
    c, o, e = run([NODE, 'collect', conf])
    out.append(('second-writer exits', [x[0] for x in b], b[0][2][:70], 'inspect-during', ic, 'A exit', a.returncode, 'after-stop collect', c, e.strip()[:60]))
for x in out: print(x)
last, dc, exact, cnt = drain_and_check(conf, 8)
print('exact', exact, cnt)
