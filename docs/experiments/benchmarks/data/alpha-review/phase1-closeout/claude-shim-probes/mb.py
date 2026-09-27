import sys
sys.path.insert(0, '/home/kmosoti/projects/fabric_o11y/.claude/worktrees/agent-ae2ceacd44d37caed/target/alpha-p15-claude-shim')
from common import *
from fab import frames
PREFIX = len('log source unavailable '.encode())
bad = 0
for ch in (chr(0x1F600), chr(0x4E2D), chr(0xE9)):
    for shift in range(0, 5):
        root = WT + '/target/alpha-p15-claude-mb'
        shutil.rmtree(root, ignore_errors=True); os.makedirs(root)
        base = root + '/'
        # choose filler so that a multibyte char straddles byte 256 of the gap text
        need = 256 - PREFIX - len(base.encode()) - shift
        filler = 'a' * max(0, need)
        name = base + filler + ch * 8
        if len(name.encode()) > 240: name = name.encode()[:240].decode('utf-8', 'ignore')
        conf = root + '/node.conf'
        with open(conf, 'w') as f:
            f.write('spool_dir=' + root + '/spool' + NL + 'log=' + name + NL + 'metric_interval_s=15' + NL + 'spool_bytes=16777216' + NL)
        c, o, e = run([NODE, 'collect', conf])
        # create the log afterwards and collect again
        with open(name, 'w') as f: f.write('late-line' + NL)
        c2, o2, e2 = run([NODE, 'collect', conf])
        fr = frames(root + '/spool/batches.faj')
        g = fr[0]['gaps'] if fr else []
        glen = [len(x.encode()) for x in g]
        full = ('log source unavailable ' + name + ': No such file or directory (os error 2)')
        ok = c == 0 and c2 == 0 and len(g) == 1 and glen[0] <= 256 and full.startswith(g[0]) and len(full.encode()) > 256 and ' logs=1 ' in o2
        bad += not ok
        print(repr(ch), shift, 'pathbytes', len(name.encode()), 'exit', c, c2, 'gap bytes', glen, 'fullbytes', len(full.encode()), 'next', o2.strip(), 'ok', ok, e.strip()[:80])
print('MB_BAD', bad)
