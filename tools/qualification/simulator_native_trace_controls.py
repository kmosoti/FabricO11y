"""Explicit finite publication controls; uses the original TLS diagnostic fixture."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import sys
import threading
import time
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from resource_group import require_limits, require_storage
import simulator_native_controls as native
from simulator_native_trace import NativeTrace, POPEN, publication, require, sha, strings, trace_calls


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--case', choices=('positive', 'write', 'sync', 'link', 'unlink', 'existing', 'stage', 'stage-symlink'), required=True)
    for option in ('binary', 'authority', 'build-receipt', 'strace', 'out'):
        parser.add_argument('--' + option, type=Path, required=True)
    for option in ('authority-sha256', 'strace-sha256'):
        parser.add_argument('--' + option, required=True)
    parser.add_argument('--calibration', type=Path)
    parser.add_argument('--calibration-sha256')
    args = parser.parse_args()
    require_limits()
    data = require_storage()
    require(args.out.parent.resolve().is_relative_to(data / 'results') and not args.out.exists(), 'control-output')
    args.out.mkdir(mode=0o700)
    report = dict(case=args.case, classification='finite native syscall publication control only', exit=2,
                  authority_sha256=args.authority_sha256, tool_sha256=args.strace_sha256)
    handles, sinks, victim, selected = [], [], None, None
    old_argv = sys.argv
    original_case = 'existing' if args.case == 'existing' else 'positive'
    fixture = args.out / 'fixture'
    deadline = time.monotonic() + 120
    previous_alarm = signal.signal(signal.SIGALRM, lambda *_: (_ for _ in ()).throw(TimeoutError('native trace controller deadline')))
    signal.alarm(120)
    class TrackedSink(native.Sink):
        loop_thread = None
        def __init__(self, *values, **options):
            super().__init__(*values, **options)
            sinks.append(self)
        def serve_forever(self, *values, **options):
            self.loop_thread = threading.current_thread()
            return super().serve_forever(*values, **options)
    try:
        if args.case in ('write', 'sync', 'link', 'unlink'):
            require(args.calibration is not None and sha(args.calibration) == args.calibration_sha256, 'calibration-pin')
            calibration = json.loads(args.calibration.read_text())
            require(calibration['authority_sha256'] == args.authority_sha256
                    and calibration['tool_sha256'] == args.strace_sha256
                    and calibration['binary_sha256'] == sha(args.binary), 'calibration-inputs')
            require(sha(Path(calibration['positive_trace'])) == calibration['positive_trace_sha256']
                    and sha(Path(calibration['positive_observer_receipt'])) == calibration['positive_observer_receipt_sha256'], 'calibration-evidence')
            selected = calibration['selectors'][args.case]
        def start(command, *positional, **options):
            nonlocal victim
            if not (isinstance(command, list) and command and command[0].startswith('/proc/self/fd/')
                    and '--server-url' in command):
                return POPEN(command, *positional, **options)
            require(not handles and not positional, 'one-target')
            fd = int(command[0].rsplit('/', 1)[1])
            require(options['pass_fds'] == (fd,), 'held-target-fd')
            injection = None if selected is None else f"{selected['syscall']}:error=EIO:when={selected['ordinal']}"
            handle = NativeTrace.start(command, fd, args.out / 'trace', strace=args.strace,
                strace_sha256=args.strace_sha256, deadline=deadline, stdout=options['stdout'],
                stderr=options['stderr'], injection=injection)
            handles.append(handle)
            if args.case in ('stage', 'stage-symlink'):
                stage = fixture / f'.begin-witness-{handle.pid}.stage'
                victim = fixture / 'preserved-victim'
                victim.write_bytes(b'preserve exact owned victim\n')
                victim.chmod(0o400)
                if args.case == 'stage-symlink': stage.symlink_to(victim)
                else: stage.write_bytes(b'preserve exact owned stage\n')
            return handle
        sys.argv = [native.__file__, '--case', original_case, '--binary', str(args.binary),
            '--authority', str(args.authority), '--authority-sha256', args.authority_sha256,
            '--build-receipt', str(args.build_receipt), '--out', str(fixture)]
        with patch.object(native.subprocess, 'Popen', start), patch.object(native, 'Sink', TrackedSink):
            fixture_exit = native.main()
        require(len(handles) == 1, 'actual-target')
        handle = handles[0]
        target_exit = handle.wait(timeout=max(0, deadline-time.monotonic()))
        original = json.loads((fixture / 'control-receipt.json').read_text())
        require(original['helper_stopped'] and original['sink_stopped'], 'fixture-stop')
        report.update(fixture_exit=fixture_exit, target_exit=target_exit, tracer_exit=handle.tracer.returncode,
                      process=handle.identity, original_fixture_receipt_sha256=sha(fixture / 'control-receipt.json'))
        raw = handle.trace.read_bytes()
        calls = trace_calls(raw)
        stage = str(fixture / f'.begin-witness-{handle.pid}.stage')
        begin = fixture / 'begin.json'
        if args.case == 'positive':
            require(fixture_exit == target_exit == 0, 'positive-fixture')
            observer = handle.finish(begin)
            offsets = observer['publication']['operation_offsets']
            selectors = {}
            for key, step in (('write', 1), ('sync', 3), ('link', 4), ('unlink', 5)):
                offset = next(value for name, value in offsets.items() if name.endswith(':' + str(step)))
                call = next(call for call in calls if call['offset'] == offset)
                selectors[key] = dict(syscall=call['name'], ordinal=sum(
                    item['name'] == call['name'] and item['offset'] <= offset for item in calls), positive_offset=offset)
            calibration = dict(schema='native-publication-calibration-v1', selectors=selectors,
                authority_sha256=args.authority_sha256, binary_sha256=sha(args.binary), tool_sha256=args.strace_sha256,
                positive_trace=str(handle.trace), positive_trace_sha256=sha(handle.trace),
                positive_observer_receipt=str(args.out / 'trace/observer-receipt.json'),
                positive_observer_receipt_sha256=sha(args.out / 'trace/observer-receipt.json'))
            (args.out / 'calibration.json').write_text(json.dumps(calibration, indent=2) + '\n')
            report.update(publication=observer['publication'], calibration_sha256=sha(args.out / 'calibration.json'))
        else:
            require(target_exit == 1 and not any(call['name'] in ('clone', 'clone3', 'fork', 'vfork') for call in calls), 'refusal-before-worker')
            if selected is not None:
                matches = [call for call in calls if '(INJECTED)' in call['tail']]
                require(len(matches) == 1, 'one-injected-operation')
                fault = matches[0]
                require(fault['name'] == selected['syscall'] and fault['result'] == -1 and 'EIO' in fault['tail']
                    and sum(call['name'] == fault['name'] and call['offset'] <= fault['offset'] for call in calls) == selected['ordinal'], 'actual-injected-operation')
                if args.case in ('write', 'sync'):
                    if args.case == 'write':
                        opened = [call for call in calls if call['name'] in ('open', 'openat')
                                  and stage in strings(call['args']) and call['result'] >= 0]
                        require(len(opened) == 1 and int(fault['args'].split(',', 1)[0], 0) == opened[0]['result'], 'injected-stage-fd')
                    else:
                        require(re.match(r'\d+<' + re.escape(stage) + r'>', fault['args']), 'injected-stage-fd')
                else:
                    require(strings(fault['args']) == ([stage, str(begin)] if args.case == 'link' else [stage]), 'injected-publication-path')
                report['injected_operation'] = fault
            elif args.case == 'existing':
                require(fixture_exit == 0 and begin.read_bytes() == b'never replace this destination\n'
                        and any(call['name'] in ('link', 'linkat') and strings(call['args']) == [stage, str(begin)]
                                and call['result'] == -1 and 'EEXIST' in call['tail'] for call in calls), 'existing-destination')
            else:
                require(any(call['name'] in ('open', 'openat') and stage in strings(call['args'])
                            and call['result'] == -1 and 'EEXIST' in call['tail'] for call in calls), 'stage-collision')
                require(victim.read_bytes() == b'preserve exact owned victim\n', 'victim-preserved')
                if args.case == 'stage': require(Path(stage).read_bytes() == b'preserve exact owned stage\n', 'stage-preserved')
            require(not any(fixture.glob('batch-*')), 'no-wire-offer')
            handle.record.update(state='expected-publication-refusal', exit=target_exit,
                tracer_exit=handle.tracer.returncode, trace_sha256=sha(handle.trace))
            handle.archive(); handle.release()
        report.update(exit=0, status='finite-control-satisfied', trace_sha256=sha(args.out / 'trace/syscalls.raw'))
    except BaseException as error:
        report['error'] = type(error).__name__ + ': ' + str(error)
    finally:
        sys.argv = old_argv
        for handle in handles:
            if handle.binary_fd is not None: handle.abort()
        for sink in sinks:
            if sink.loop_thread is not None and sink.loop_thread.is_alive():
                sink.shutdown(); sink.loop_thread.join(timeout=5)
            sink.server_close()
            if sink.loop_thread is not None and sink.loop_thread.is_alive():
                report.update(exit=2, cleanup_error='owned TLS sink not stopped')
        signal.alarm(0)
        signal.signal(signal.SIGALRM, previous_alarm)
        (args.out / 'receipt.json').write_text(json.dumps(report, indent=2) + '\n')
    return report['exit']


if __name__ == '__main__':
    raise SystemExit(main())
