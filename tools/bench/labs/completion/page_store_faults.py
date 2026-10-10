#!/usr/bin/env python3
"""Real page-file faults with evidence captured before recovery can clean it."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
from resource_group import STORAGE, require_limits


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def leftovers(state):
    return sorted(str(path.relative_to(state)) for path in state.rglob('*')
                  if path.name.startswith('.building') or '.run-' in path.name or path.suffix == '.pages')


def main():
    require_limits()
    parser = argparse.ArgumentParser()
    parser.add_argument('--builder', type=Path, required=True)
    parser.add_argument('--helper', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    if args.out.exists() or not args.out.parent.resolve().is_relative_to(STORAGE.resolve()):
        raise RuntimeError('fresh data-drive results required')
    census = subprocess.run(['du', '-sx', '-B1', str(STORAGE)], capture_output=True, text=True, check=True)
    if int(census.stdout.split()[0]) + 2 * 1024**3 > 100_000_000_000:
        raise RuntimeError('100 GB allowance cannot admit page fault reserve')
    args.out.mkdir()
    for name in ('builder', 'helper'):
        frozen_binary = args.out / ('frozen-' + name)
        shutil.copy2(getattr(args, name), frozen_binary)
        setattr(args, name, frozen_binary)
    work = Path(os.environ['FABRIC_SCRATCH_ROOT']) / args.out.name
    work.mkdir()
    library = args.out / 'io_fault.so'
    compile_command = ['gcc', '-shared', '-fPIC', '-O2', '-Wall', '-o', str(library),
                       str(Path(__file__).with_name('io_fault.c')), '-ldl']
    compiled = subprocess.run(compile_command, capture_output=True, text=True)
    write(args.out / 'compile.json', {'command': compile_command, 'exit': compiled.returncode,
                                    'stdout': compiled.stdout, 'stderr': compiled.stderr})
    if compiled.returncode:
        raise RuntimeError('fault library compilation failed')
    frozen = {str(path): sha(path) for path in (args.builder, args.helper, library,
              Path(__file__), Path(__file__).with_name('io_fault.c'))}
    write(args.out / 'provenance.json', {'command': sys.argv, 'sha256': frozen,
                                        'storage_census': census.stdout})
    clean = {key: value for key, value in os.environ.items() if not key.startswith('FABRIC_FAULT_')}
    generated = subprocess.run([str(args.builder), 'gen', str(work / 'fixture'), 'steady', '16'],
                               capture_output=True, text=True, env=clean, timeout=120)
    write(args.out / 'generate.json', {'command': [str(args.builder), 'gen', str(work / 'fixture'), 'steady', '16'],
                                     'exit': generated.returncode, 'stdout': generated.stdout, 'stderr': generated.stderr})
    if generated.returncode:
        raise RuntimeError('fixture failed')
    fixture = json.loads(generated.stdout)
    source = Path(fixture['input'])

    def run(label, state, mode, environment, pause=False):
        command = [str(args.helper), str(state), str(source), mode]
        before = {path: sha(Path(path)) for path in frozen}
        if before != frozen:
            raise RuntimeError('binary/library changed before child')
        stdout_path, stderr_path = args.out / (label + '.stdout'), args.out / (label + '.stderr')
        with stdout_path.open('w') as stdout, stderr_path.open('w') as stderr:
            child = subprocess.Popen(command, stdout=stdout, stderr=stderr, env=environment)
            try:
                if pause:
                    deadline = time.monotonic() + 60
                    while time.monotonic() < deadline:
                        text = stderr_path.read_text()
                        hit = re.search(r'FABRIC_INJECTION op=read path=(\S+) match=', text)
                        status_path = Path('/proc') / str(child.pid) / 'status'
                        if hit and status_path.exists() and re.search(r'^State:\s+T', status_path.read_text(), re.M):
                            blob = Path(hit.group(1))
                            if not blob.is_relative_to(state) or blob.suffix != '.pages':
                                raise RuntimeError('unexpected paused page path')
                            # Keep the readable open inode, replace its pathname with a
                            # directory: native last-take unlink must reject this.
                            blob.unlink()
                            blob.mkdir()
                            child.send_signal(signal.SIGCONT)
                            break
                        if child.poll() is not None:
                            raise RuntimeError('cleanup fault did not stop at page read')
                        time.sleep(0.05)
                    else:
                        raise RuntimeError('cleanup injection timeout')
                code = child.wait(timeout=120)
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait()
        receipt = {'command': command, 'exit': code, 'fault_environment':
                   {key: value for key, value in environment.items() if key.startswith('FABRIC_FAULT_')},
                   'hashes_after': {path: sha(Path(path)) for path in frozen}}
        write(args.out / (label + '.command.json'), receipt)
        if receipt['hashes_after'] != frozen:
            raise RuntimeError('binary/library changed after child')
        return code, stdout_path.read_text(), stderr_path.read_text()

    code, stdout, _ = run('control', work / 'control', 'build', clean)
    if code:
        raise RuntimeError('clean control failed')
    expected = json.loads(stdout)
    miss = dict(clean, LD_PRELOAD=str(library), FABRIC_FAULT_ROOT=str(work),
                FABRIC_FAULT_MATCH='DOES-NOT-EXIST', FABRIC_FAULT_OP='write')
    code, stdout, stderr = run('no-hit', work / 'no-hit', 'build', miss)
    if code or 'FABRIC_INJECTION' in stderr or json.loads(stdout) != expected:
        raise RuntimeError('no-hit negative control failed')
    outcomes = []
    for label, operation in [('write', 'write'), ('read', 'read'), ('partial-write', 'write'),
                             ('partial-read', 'read'), ('kill-write', 'write'),
                             ('kill-read', 'read'), ('cleanup', 'read')]:
        state = work / label
        environment = dict(clean, LD_PRELOAD=str(library), FABRIC_FAULT_ROOT=str(work),
                           FABRIC_FAULT_MATCH='/logs.pages/', FABRIC_FAULT_OP=operation)
        killed = label.startswith('kill-')
        if killed: environment['FABRIC_FAULT_KILL'] = '1'
        if label.startswith('partial-'): environment['FABRIC_FAULT_PARTIAL'] = '1'
        if label == 'cleanup': environment['FABRIC_FAULT_PAUSE'] = '1'
        code, _, stderr = run(label, state, 'build', environment, pause=label == 'cleanup')
        outcome = {'case': label, 'exit': code, 'injected': 'FABRIC_INJECTION' in stderr,
                   'input_unchanged': sha(source) == fixture['input_sha256'],
                   'before_recovery_leftovers': leftovers(state),
                   'before_recovery_files': sorted(str(path.relative_to(state)) for path in state.rglob('*') if path.is_file())}
        write(args.out / (label + '.before-recovery.json'), outcome)
        retry, recovered, _ = run(label + '-retry', state, 'recover', clean)
        outcome.update(retry_exit=retry, exact_manifest=retry == 0 and json.loads(recovered) == expected,
                       after_recovery_leftovers=leftovers(state))
        outcome['passed'] = (outcome['injected'] and code != 0 and (not killed or code == -9)
                             and outcome['input_unchanged'] and retry == 0 and outcome['exact_manifest']
                             and not outcome['after_recovery_leftovers']
                             and (killed or not outcome['before_recovery_leftovers']))
        outcomes.append(outcome)
        write(args.out / 'outcomes.json', outcomes)
    if not all(outcome['passed'] for outcome in outcomes):
        raise RuntimeError('page fault rejected; preserve scratch evidence')
    shutil.rmtree(work)
    write(args.out / 'cleanup.json', {'removed': not work.exists(), 'owned_scratch': str(work)})
    print(json.dumps({'passed': True, 'cases': len(outcomes), 'result': str(args.out)}))


if __name__ == '__main__':
    main()
