import sys, time
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common_rot2 import *
SHIM2 = WT + '/target/alpha-p15-claude-shim/shim2.so'
root = WT + '/target/alpha-p15-claude-rot'
spool = root + '/spool'
base = WT + '/target/alpha-p15-claude-rot-base'
conf = root + '/node.conf'
N = 16 * 140
def restore():
    shutil.rmtree(spool); shutil.copytree(base, spool)
def lines_ok():
    last, dc, exact, cnt = drain_and_check(conf, N)
    return last, exact, cnt
# 1) second writer during the rotation window
for at in (13, 14):
    restore()
    env = dict(os.environ); env.update(LD_PRELOAD=SHIM2, SHIM_PREFIX=spool, SHIM_AT=str(at), SHIM_MODE='sleep_before', SHIM_SLEEP_MS='1500')
    a = subprocess.Popen([NODE, 'collect', conf], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env)
    time.sleep(0.7)
    b = run([NODE, 'collect', conf])
    ao, ae = a.communicate(timeout=20)
    files = sorted(os.listdir(spool))
    ic, d, ie = inspect(conf)
    last, exact, cnt = lines_ok()
    print('RACE at', at, '| A exit', a.returncode, ae.strip()[:90], '| B exit', b[0], b[2].strip()[:90], '| files', files, '| inspect', ic, d.get('recovery_required'), ie[:60], '| drain', last[0], last[2][:60], 'exact', exact, cnt)
# 2) SIGKILL at each rotation event
for mode in ('kill_before', 'kill_after'):
    for at in range(10, 16):
        restore()
        c, o, e = run([NODE, 'collect', conf], env=shim_env(root, at=at, mode=mode))
        files = sorted(os.listdir(spool))
        ic, d, ie = inspect(conf)
        last, exact, cnt = lines_ok()
        print('KILL', mode, at, '| exit', c, '| files', files, '| inspect', ic, d.get('recovery_required'), ie[:60], '| drain', last[0], last[2][:80], 'exact', exact, cnt)
