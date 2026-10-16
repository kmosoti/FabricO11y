"""Finite native guardian controls; never product S1 or soak acceptance."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from resource_guardian import identity
from resource_group import STORAGE, require_limits
from resource_final_group import FIELDS, require


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--case', choices=('positive', 'unexited', 'populated', 'wrong-identity', 'escaped'), required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    signal.alarm(60)
    group = require_limits()
    require(os.environ.get('FABRIC_RESOURCE_TOPOLOGY') == 'guardian-v1'
            and group.name == 'workload' and group.parent.name.startswith('fabric-work-'),
            'actual guardian workload root required')
    require(not args.out.exists() and args.out.is_absolute(), 'fresh output required')
    args.out.parent.resolve(strict=True).relative_to(STORAGE / 'results')
    args.out.mkdir(mode=0o700)
    record = {'case': args.case, 'process': identity(os.getpid()), 'wrapper': identity(os.getppid()),
              'fields': {key: (group / key).read_text().strip() for key in FIELDS},
              'at_ns': time.monotonic_ns(), 'scope': 'unqualified guardian control only'}
    if args.case == 'positive':
        from resource_guardian import workload_context
        sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'bench/labs/completion'))
        import cgroups
        context = workload_context()
        require(context is not None and context['workload'] == group, 'native held runtime context failed')
        require(cgroups.delegate() == group, 'native held workload delegation failed')
        cgroups.set_limits(group / 'supervisor', 2 * 1024**3, 1700 * 1024**2, 512, 4)
        require(workload_context()['workload'] == group and cgroups.current() == group / 'supervisor',
                'native delegated caller left authenticated workload')
        record['runtime_context'] = {'unit': context['unit'], 'activation_monotonic_ns': context['activation_monotonic_ns'],
                                    'delegated_process': identity(os.getpid()), 'groups': cgroups.snapshot(group)}
    if args.case in ('populated', 'escaped'):
        code = 'import time; time.sleep(120)'
        if args.case == 'escaped':
            code = ('from pathlib import Path; import os,time; '
                    f'Path({str(group.parent / "control-plane/cgroup.procs")!r}).write_text(str(os.getpid())); '
                    'print("escaped",flush=True); time.sleep(120)')
        child = subprocess.Popen([sys.executable, '-B', '-c', code], stdout=subprocess.PIPE, text=True)
        if args.case == 'escaped':
            require(child.stdout.readline().strip() == 'escaped', 'actual escape did not complete')
        record['owned_child'] = identity(child.pid)
        require(child.poll() is None, 'native idle child unexpectedly exited')
    if args.case == 'unexited':
        fields = (Path('/proc') / str(os.getppid()) / 'cmdline').read_bytes().split(b'\0')
        index = fields.index(b'--guarded-inside')
        receipt = Path(os.fsdecode(fields[index + 3]))
        limit = time.monotonic() + 2
        while not receipt.exists() and time.monotonic() < limit:
            time.sleep(.01)
        forged = json.loads(receipt.read_text())
        forged.update(state='completed', exit=0)
        receipt.write_text(json.dumps(forged))
        record['forged_completed_while_wrapper_live_ns'] = time.monotonic_ns()
    (args.out / 'native.json').write_text(json.dumps(record, indent=2) + '\n')
    if args.case == 'unexited':
        time.sleep(30)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
