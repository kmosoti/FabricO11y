"""Current-source build and bounded syscall/repeated-recovery investigation."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools'))
sys.path.insert(0, str(ROOT / 'tools/bench/labs/cross_system'))
from resource_group import require_limits
import native_snapshot

PROTOCOL = ROOT / 'docs/experiments/benchmarks/service-recovery-research-protocol.md'
BASE = ROOT / 'docs/experiments/benchmarks/data/native-frontier-01/memory/service-recovery'


def dump(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def execute(argv, out, name, deadline, env=None):
    timeout = deadline - time.monotonic()
    if timeout <= 0:
        raise TimeoutError('absolute research deadline')
    started = time.monotonic()
    with (out / (name + '.stdout')).open('wb') as stdout, (out / (name + '.stderr')).open('wb') as stderr:
        result = subprocess.run(list(map(str, argv)), stdout=stdout, stderr=stderr,
                                env=env, timeout=timeout, cwd=ROOT)
    dump(out / (name + '.command.json'), dict(argv=list(map(str, argv)), exit=result.returncode,
         elapsed_s=time.monotonic()-started,
         fault_env={k: v for k, v in (env or {}).items() if k.startswith('FABRIC_FAULT_')}))
    return result.returncode


def build(out, deadline):
    sources = sorted({p for directory in (ROOT/'src', ROOT/'crates', ROOT/'examples')
                      for p in directory.rglob('*.rs')} |
                     {ROOT/'Cargo.toml', ROOT/'Cargo.lock', PROTOCOL, Path(__file__)} |
                     set((ROOT/'crates').glob('*/Cargo.toml')) |
                     set((ROOT/'tools/bench/labs/readiness_service').glob('*.py')) |
                     {ROOT/'tools/bench/labs/completion/io_fault.c'})
    identities = {str(p.relative_to(ROOT)): sha(p) for p in sources}
    env = dict(os.environ, CARGO_BUILD_JOBS='2')
    # Normal production feature set and allocator policy; opt-ins are build-time.
    for key in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'LD_PRELOAD', 'MALLOC_MMAP_THRESHOLD_'):
        env.pop(key, None)
    commands = [
        ['cargo', 'build', '--offline', '--locked', '--release', '-p', 'fabric_o11y', '--bin', 'fabric-node'],
        ['cargo', 'build', '--offline', '--locked', '--release', '-p', 'fabric-server', '--bin', 'fabric-server',
         '--example', 'server_dump', '--example', 'completion_builder', '--example', 'completion_fault'],
    ]
    for n, command in enumerate(commands):
        if execute(command, out, f'build-{n}', deadline, env):
            raise RuntimeError('current-source build failed')
    if identities != {str(p.relative_to(ROOT)): sha(p) for p in sources}:
        raise RuntimeError('build source changed during compilation')
    bins = Path(os.environ['CARGO_TARGET_DIR'])/'release'
    names = ['fabric-node', 'fabric-server', 'examples/server_dump',
             'examples/completion_builder', 'examples/completion_fault']
    manifest = dict(source_sha256=identities,
                    binaries={name: sha(bins/name) for name in names},
                    bin_dir=str(bins), protocol_sha256=sha(PROTOCOL),
                    revision=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
                    commands=commands, build_jobs=2, experimental_features=False)
    dump(out/'build.json', manifest)
    shutil.copyfile(PROTOCOL, out/'protocol.txt')
    old = sys.argv
    try:
        sys.argv = ['native_snapshot.py', '--build-manifest', str(out/'build.json'), '--out', str(out/'sources')]
        native_snapshot.main()
    finally:
        sys.argv = old


def decision(row):
    return (row['injected'] and row['failed_exit'] != 0 and row['input_unchanged']
            and row['retry_exit'] == 0 and row['exact_manifest'] and not row['leftovers'])


def controls():
    good = dict(injected=True, failed_exit=1, input_unchanged=True, retry_exit=0,
                exact_manifest=True, leftovers=[])
    assert decision(good)
    rejected = []
    for key, value in [('injected', False), ('input_unchanged', False),
                       ('exact_manifest', False), ('leftovers', ['.building'])]:
        assert not decision(dict(good, **{key: value}))
        rejected.append(key)
    return dict(positive=True, rejected=rejected)


def files(root):
    result = {}
    for path in sorted(root.rglob('*')):
        if path.is_symlink():
            raise RuntimeError('linked fixture')
        if path.is_file():
            result[str(path.relative_to(root))] = dict(bytes=path.stat().st_size, sha256=sha(path))
    return result


def faults(out, deadline):
    work = Path(os.environ['FABRIC_SCRATCH_ROOT'])/'recovery'
    work.mkdir()
    bins = Path(os.environ['CARGO_TARGET_DIR'])/'release/examples'
    helper, generator = bins/'completion_fault', bins/'completion_builder'
    manifest = json.loads((BASE/'build/build.json').read_text())
    for name in ('completion_fault', 'completion_builder'):
        if sha(bins/name) != manifest['binaries']['examples/'+name]:
            raise RuntimeError('changed helper binary')
    library = work/'fault.so'
    if execute(['gcc', '-shared', '-fPIC', '-O2', '-Wall', '-o', library,
                ROOT/'tools/bench/labs/completion/io_fault.c', '-ldl'], out, 'interposer', deadline):
        raise RuntimeError('interposer build')
    if execute([generator, 'gen', work/'input', 'steady', '16'], out, 'generate', deadline):
        raise RuntimeError('fixture generation')
    source = Path(json.loads((out/'generate.stdout').read_text())['input'])
    original = sha(source)
    clean = dict(os.environ)
    clean.pop('LD_PRELOAD', None)
    def run(label, state, mode, env):
        return execute([helper, state, source, mode], out, label, deadline, env)
    state = work/'control'
    if run('control', state, 'build', clean):
        raise RuntimeError('fault-free baseline failed')
    expected = json.loads((out/'control.stdout').read_text())
    dump(out/'control-files.json', files(state))
    shutil.rmtree(state)
    miss = dict(clean, LD_PRELOAD=str(library), FABRIC_FAULT_ROOT=str(work),
                FABRIC_FAULT_MATCH='DOES-NOT-EXIST', FABRIC_FAULT_OP='write')
    if run('unmatched-control', work/'unmatched', 'build', miss) or 'FABRIC_INJECTION' in (out/'unmatched-control.stderr').read_text():
        raise RuntimeError('unmatched control claimed a failure')
    shutil.rmtree(work/'unmatched')
    cases = [('spill-write','write','.run-0-',False), ('merge-read','read','.run-0-',False),
        ('raw-write','write','batches.parquet',False), ('logs-write','write','logs.parquet',False),
        ('metrics-write','write','metrics.parquet',False), ('filter-write','write','text_filter.bin',False),
        ('manifest-write','write','manifest.json',False), ('table-sync','sync','logs.parquet',False),
        ('manifest-sync','sync','manifest.json',False), ('publication','rename','.building-00000000000000000001$',False),
        ('published-dir-sync','sync','/segments$',False), ('kill-spill','write','.run-0-',True),
        ('kill-manifest','sync','manifest.json',True), ('kill-before-publication','rename','.building-00000000000000000001$',True),
        ('journal-read','read','sealed-',False), ('partial-spill-write','write','.run-0-',False),
        ('partial-merge-read','read','.run-0-',False), ('repeated-recovery','write','.run-0-',True)]
    outcomes = []
    for label, op, pattern, kill in cases:
        state = work/label
        env = dict(clean, LD_PRELOAD=str(library), FABRIC_FAULT_ROOT=str(work),
                   FABRIC_FAULT_MATCH=pattern, FABRIC_FAULT_OP=op)
        if kill:
            env['FABRIC_FAULT_KILL'] = '1'
        if label.startswith('partial-'):
            env['FABRIC_FAULT_PARTIAL'] = '1'
        code = run(label, state, 'build', env)
        row = dict(case=label, injected='FABRIC_INJECTION' in (out/(label+'.stderr')).read_text(),
                   failed_exit=code, input_unchanged=sha(source)==original,
                   before_retry=files(state), repeated_cuts=[])
        if label == 'repeated-recovery':
            again = dict(env, FABRIC_FAULT_OP='rename', FABRIC_FAULT_MATCH='.building-00000000000000000001$')
            for n in range(2):
                cut = label+f'-cut-{n}'
                cut_code = run(cut, state, 'recover', again)
                hit = 'FABRIC_INJECTION' in (out/(cut+'.stderr')).read_text()
                row['repeated_cuts'].append(dict(exit=cut_code, injected=hit, files=files(state)))
                if cut_code != -9 or not hit or sha(source) != original:
                    raise RuntimeError('repeated recovery cut did not occur exactly')
        code = run(label+'-retry', state, 'recover', clean)
        row.update(retry_exit=code,
                   exact_manifest=code==0 and json.loads((out/(label+'-retry.stdout')).read_text())==expected,
                   leftovers=[str(p.relative_to(state)) for p in state.rglob('*') if p.name.startswith('.building') or p.name.startswith('.run-')],
                   after_retry=files(state))
        row['input_unchanged'] &= sha(source)==original
        row['passed'] = decision(row)
        outcomes.append(row)
        dump(out/'outcomes.json', outcomes)
        if not row['passed']:
            # Keep the complete current failed state; launcher will retain it.
            raise RuntimeError('storage failure counterexample: '+label)
        shutil.rmtree(state)
    dump(out/'summary.json', dict(cases=len(outcomes), passed=all(x['passed'] for x in outcomes),
         source_sha256=original, controls=controls(), injection='ENOSPC/EIO/SIGKILL; no physical disk or power fault'))
    shutil.rmtree(work)
    dump(out/'cleanup.json', dict(scratch=str(work), removed=not work.exists()))


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('mode', choices=['build', 'faults', 'controls'])
    ap.add_argument('--id', required=True)
    ap.add_argument('--seconds', type=int, default=550)
    args = ap.parse_args()
    require_limits()
    if os.environ.get('FABRIC_NATIVE_COORDINATED') != '1':
        raise RuntimeError('native coordinator required')
    out = BASE/args.id
    out.mkdir(parents=True, exist_ok=False)
    shutil.copyfile(PROTOCOL, out/'protocol.txt')
    shutil.copyfile(Path(__file__), out/'research.py')
    if args.mode == 'controls':
        dump(out/'controls.json', controls())
    elif args.mode == 'build':
        build(out, time.monotonic()+args.seconds)
    else:
        faults(out, time.monotonic()+args.seconds)


if __name__ == '__main__':
    main()
