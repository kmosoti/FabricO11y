"""SIGKILL fabric-node collect at random points and check exact recovery.

Each iteration appends one numbered line to a source log, starts one
`fabric-node collect`, and kills it after a random 0-12 ms delay (iteration 0 is not killed). After every
iteration `fabricctl inspect` must succeed (a spool killed during its first
open may still lack an identity). Final clean collects, repeated until one commits no new record, must succeed and the
persisted log bodies must equal every written line exactly once, in order.
Disposable data lives under target/alpha-kill-probe-<seed>/ and is removed
after a pass.
"""

import argparse
import os
import random
import shutil
import signal
import subprocess
import sys
import time


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--bin-dir", default="target/release")
    parser.add_argument("--seed", type=int, default=7)
    parser.add_argument("--iterations", type=int, default=300)
    args = parser.parse_args()
    node = os.path.join(args.bin_dir, "fabric-node")
    ctl = os.path.join(args.bin_dir, "fabricctl")
    dump = os.path.join(args.bin_dir, "examples", "native_dump")
    root = os.path.abspath(f"target/alpha-kill-probe-{args.seed}")
    if os.path.islink(root):
        raise SystemExit("refusing symlinked probe root")
    shutil.rmtree(root, ignore_errors=True)
    os.makedirs(root)
    log = os.path.join(root, "app.log")
    conf = os.path.join(root, "node.conf")
    open(log, "w").close()
    with open(conf, "w") as out:
        out.write(
            f"spool_dir={root}/spool\nlog={log}\nmetric_interval_s=15\nspool_bytes=16777216\n"
        )
    rng = random.Random(args.seed)
    killed = interrupted = 0
    for i in range(args.iterations):
        with open(log, "a") as out:
            out.write(f"line-{i:05d}\n")
        child = subprocess.Popen([node, "collect", conf], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        time.sleep(rng.uniform(0, 0.012))
        # Iteration 0 creates the spool; the first-open kill case has its own
        # unit test. Later kills target appends to an existing journal.
        if i > 0 and child.poll() is None:
            child.send_signal(signal.SIGKILL)
            killed += 1
        child.communicate()
        if i == 0:
            child_exit = child.returncode
            if child_exit != 0:
                print(f"FAIL first collect exit={child_exit}")
                return 1
        status = subprocess.run([ctl, "inspect", conf], capture_output=True, text=True)
        if status.returncode != 0 and "missing journal identity" in status.stderr:
            continue
        if status.returncode != 0 or "recovery_required=true" in status.stdout:
            print(f"FAIL iteration={i} inspect_exit={status.returncode} {status.stdout.strip()} {status.stderr.strip()}")
            return 1
        interrupted += "interrupted_append=true" in status.stdout
    # Drain: one pass reads at most 128 lines per file, and killed cycles
    # leave a backlog. Collect until a cycle commits no new log record.
    for _ in range(args.iterations // 128 + 2):
        final = subprocess.run([node, "collect", conf], capture_output=True, text=True)
        if final.returncode != 0 or " logs=0 " in final.stdout:
            break
    replay = subprocess.run([dump, conf], capture_output=True, text=True)
    bodies = [line for line in replay.stdout.splitlines() if line.startswith("line-")]
    expected = [f"line-{n:05d}" for n in range(args.iterations)]
    exact = final.returncode == 0 and replay.returncode == 0 and bodies == expected
    print(
        f"seed={args.seed} iterations={args.iterations} killed={killed} "
        f"interrupted_seen={interrupted} final_exit={final.returncode} "
        f"replay_exit={replay.returncode} lines={len(bodies)}/{args.iterations} exact={exact}"
    )
    if not exact:
        print(final.stderr.strip())
        return 1
    shutil.rmtree(root)
    return 0


if __name__ == "__main__":
    sys.exit(main())
