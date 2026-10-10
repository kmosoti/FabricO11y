#!/usr/bin/env python3
"""Guest-local data-only interruption, independent of host query pacing."""
import argparse
import json
from pathlib import Path
import subprocess
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--begin-ns', type=int, required=True)
    parser.add_argument('--port', type=int, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    if not 1 <= args.port <= 65535 or args.begin_ns <= 0:
        raise ValueError('invalid declared interruption inputs')
    boot = Path('/proc/sys/kernel/random/boot_id').read_text().strip()
    installed = False
    def run(arguments):
        return subprocess.run(['nft', *arguments], check=True, capture_output=True, text=True, timeout=10)
    with args.out.open('x') as output:
        def record(kind, before, after, **extra):
            output.write(json.dumps({'type': kind, 'boot_id': boot, 'before_ns': before,
                                     'after_ns': after, **extra}) + '\n')
            output.flush()
        try:
            time.sleep(max(0, (args.begin_ns + 135 * 10**9 - time.monotonic_ns()) / 1e9))
            before = time.monotonic_ns()
            run(['add', 'table', 'inet', 'fabric_cross'])
            installed = True
            run(['add', 'chain', 'inet', 'fabric_cross', 'output', '{ type filter hook output priority 0; policy accept; }'])
            run(['add', 'rule', 'inet', 'fabric_cross', 'output', 'ip', 'daddr', '10.0.2.2',
                 'tcp', 'dport', str(args.port), 'counter', 'drop'])
            record('outage_start', before, time.monotonic_ns())
            time.sleep(max(0, (args.begin_ns + 195 * 10**9 - time.monotonic_ns()) / 1e9))
            before = time.monotonic_ns()
            counters = json.loads(run(['-j', 'list', 'table', 'inet', 'fabric_cross']).stdout)
            run(['delete', 'table', 'inet', 'fabric_cross'])
            installed = False
            record('outage_end', before, time.monotonic_ns(), firewall_counters=counters)
        finally:
            if installed:
                run(['delete', 'table', 'inet', 'fabric_cross'])
                record('interrupted_cleanup', time.monotonic_ns(), time.monotonic_ns())


if __name__ == '__main__':
    main()
