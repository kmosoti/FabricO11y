#!/usr/bin/env python3
"""Registered native key-first screen/held-out confirmation; unchanged query oracle."""
import argparse
import copy
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import statistics
import subprocess
import sys
import time

import query_census as c
import run_native_job as n

ROOT, MIB = c.ROOT, 1024**2
SHAPES = ('absent', 'selective', 'common', 'broad', 'empty-literal', 'unicode')


def dump(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + '\n')


def bounds(work, out, reserve=True):
    if n.footprint(n.BASE/'query') + (192*MIB if reserve else 0) > 384*MIB:
        raise RuntimeError('384MiB query cap/192MiB complete failure reserve unavailable')
    if n.footprint(work) > 8*1024**3 or shutil.disk_usage(work).free < 16*1024**3:
        raise RuntimeError('8GiB scratch/16GiB free reserve unavailable')


def run(argv, env, stdout, stderr, deadline, work, out, raw=None):
    receipt = dict(argv=argv, exit=None)
    with stdout.open('wb') as so, stderr.open('wb') as se:
        child = subprocess.Popen(argv, cwd=ROOT, env=env, stdout=so, stderr=se, start_new_session=True)
        try:
            while child.poll() is None:
                bounds(work, out)
                if raw and raw.exists() and n.footprint(raw) >= 480*MIB:
                    raise RuntimeError('raw case480MiB stop threshold; preserve full case')
                if time.monotonic() >= deadline:
                    raise RuntimeError('registered build/run deadline exceeded')
                time.sleep(.1)
        finally:
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=10)
            receipt['exit'] = child.returncode
            dump(stdout.with_suffix('.command.json'), receipt)
    if child.returncode:
        raise RuntimeError(f'child exit{child.returncode}; preserve full state')


def grade(trial, evidence, pool):
    records, bodies = c.grader.decode_records(trial/'records.jsonl')
    if len(bodies) != 2048:
        raise RuntimeError('native source cardinality differs')
    results = json.loads((evidence/'native.out').read_text())
    fixture = results['fixture']
    if any(len(body.encode()) != fixture['width'] for body in bodies):
        raise RuntimeError('native source width differs')
    measures = results['results']
    expected_measurements = {(layout,plan,shape,i) for layout in ('tail','segment')
        for plan in ('scan','walk') for shape in SHAPES for i in range(4)}
    observed = [(m['layout'],m['plan'],m['shape'],m['iteration']) for m in measures]
    if len(observed) != 96 or set(observed) != expected_measurements:
        raise RuntimeError('native measured-call coverage differs')
    mappings, verdicts, rejected, metrics = {}, [], [], {}
    for layout in ('tail','segment'):
        for plan in ('scan','walk'):
            for shape in SHAPES:
                name = f'{layout}-{plan}-{shape}'
                rows = [m for m in measures if (m['layout'],m['plan'],m['shape']) == (layout,plan,shape)]
                rows.sort(key=lambda m:m['iteration'])
                queries = [m['query'] for m in rows]
                if any(query != queries[0] for query in queries):
                    raise RuntimeError('query varies within call population')
                query = queries[0]
                if query != expected_query(shape):
                    raise RuntimeError('native query contract differs from registration')
                raw_pages = (trial/f'chain-{name}.jsonl').read_bytes().splitlines(keepends=True)
                pages = [json.loads(raw) for raw in raw_pages]
                verdict = c.grader.query_oracle.check(records,query,pages)
                if not verdict['passed']:
                    dump(evidence/'oracle-failure.json',dict(query=query,verdict=verdict,pages=pages))
                    raise RuntimeError('unchanged independent complete-pagination oracle rejected')
                expected_rows = {'absent':0,'selective':128,'common':1024,'broad':2048,
                    'empty-literal':2048,'unicode':2048}[shape]
                if verdict['expected_rows'] != expected_rows or len(pages) != max(1,(expected_rows+63)//64):
                    raise RuntimeError('registered cardinality/full chain bound differs')
                verdicts.append(dict(name=name,**verdict))
                calls = (trial/f'calls-{name}.jsonl').read_bytes().splitlines(keepends=True)
                if len(calls) != 4 or any(raw != raw_pages[0] for raw in calls):
                    raise RuntimeError('measured exact first page differs from full chain')
                if len(pages) > 1:
                    defects = {'truncated_chain':pages[:-1], 'duplicated_row':copy.deepcopy(pages),
                        'changed_snapshot':copy.deepcopy(pages)}
                    defects['duplicated_row'][0]['rows'].append(copy.deepcopy(pages[0]['rows'][0]))
                    defects['changed_snapshot'][-1]['snapshot'] = 'g999-999'
                    for defect,bad in defects.items():
                        answer = c.grader.query_oracle.check(records,query,bad)
                        if answer['passed']:
                            raise RuntimeError('independent oracle accepted '+defect)
                        vector = c.profile.retain_bytes(json.dumps(bad,ensure_ascii=False).encode(),pool,'.counterexample.json')
                        rejected.append(dict(name=name,defect=defect,verdict=answer,vector=vector,query=query))
                refs = [c.profile.retain_bytes(raw[:-1],pool,'.answer.json') for raw in raw_pages]
                mappings[name] = dict(query=query,pages=refs,calls=[dict(iteration=i,first=refs[0],
                    raw_sha256=hashlib.sha256(raw).hexdigest()) for i,raw in enumerate(calls)])
                for boundary, calls_rows in (('first',rows[:1]),('reuse',rows[1:])):
                    metric = {key:statistics.median(row[key] for row in calls_rows) for key in ('cpu_ns','wall_ns')}
                    metric['requested'] = (statistics.median(row['allocation']['total'] for row in calls_rows)
                        if fixture['counted'] else None)
                    metric['peak'] = (statistics.median(row['allocation']['peak']-row['allocation']['base'] for row in calls_rows)
                        if fixture['counted'] else None)
                    metrics[name+'-'+boundary] = metric
    ledger = c.profile.retain_ledger((trial/'records.jsonl').read_bytes(),pool)
    dump(evidence/'maps.json',dict(records=ledger,chains=mappings))
    dump(evidence/'oracle.json',dict(verdicts=verdicts,rejected_controls=rejected,
        origin='unchanged tools/qualification/query_oracle.py; full raw source decoder and chains'))
    return dict(fixture=fixture,metrics=metrics,source_sha256=c.grader.sha(trial/'records.jsonl'),
        canonical_chains=len(verdicts),canonical_pages=sum(len(m['pages']) for m in mappings.values()),
        measured_first_pages=96,negative_controls=len(rejected))


def expected_query(shape):
    query = dict(kind='logs',from_ns=1600000000000000000,to_ns=1600000000000002048,limit=64)
    if shape != 'broad':
        query['contains'] = {'absent':'NO-HIT!','selective':'SEL!','common':'COM!',
            'empty-literal':'','unicode':'λ'}[shape]
    return query


def preserve_case(trial, out, pool, work, deadline):
    """Exact complete native state precedes successful case deletion."""
    size = n.footprint(trial)
    if size > 512*MIB:
        raise RuntimeError('case exceeds full512MiB raw preservation envelope')
    manifest = {}
    for path in sorted(trial.rglob('*')):
        if not path.is_file(): continue
        if time.monotonic() >= deadline: raise RuntimeError('preservation deadline exceeded')
        raw = path.read_bytes()
        # Bound the worst-case new gzip before writing; do not spend the failure reserve.
        if n.footprint(n.BASE/'query') + len(raw) + MIB + 192*MIB > 384*MIB:
            raise RuntimeError('full-state object would consume failure reserve')
        ref = c.profile.retain_bytes(raw,pool,'.native-state')
        target = pool/ref
        with gzip.open(target,'rb') as stream:
            if stream.read() != raw: raise RuntimeError('native-state exact gzip readback differs')
        manifest[str(path.relative_to(trial))] = dict(bytes=len(raw),sha256=hashlib.sha256(raw).hexdigest(),object=ref)
        bounds(work,out)
    dump(out/'native-state-manifest.json',manifest)
    # Independent second complete readback precedes cleanup, including source and native tables.
    for name, entry in manifest.items():
        with gzip.open(pool/entry['object'],'rb') as stream:
            decoded = stream.read()
        if decoded != (trial/name).read_bytes() or hashlib.sha256(decoded).hexdigest() != entry['sha256']:
            raise RuntimeError('second native-state readback differs')
    dump(out/'native-state-readback.json',dict(raw_allocated_bytes=size,members=len(manifest),
        exact_first_readback=True,exact_second_readback=True,representation='lossless content-addressed gzip member map'))


def main():
    c.profile.require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out',type=Path,required=True)
    parser.add_argument('--mode',choices=('controls','screen','confirm'),default='screen')
    parser.add_argument('--binaries',type=Path)
    parser.add_argument('--screen-admission',type=Path,help='root-authored nomination receipt authenticating screen result')
    parser.add_argument('--build-seconds',type=int,default=600)
    parser.add_argument('--run-seconds',type=int,default=600)
    args = parser.parse_args()
    if not 0 < args.build_seconds <= 600 or not 0 < args.run_seconds <= 900:
        parser.error('build<=600/run<=900 seconds')
    if args.mode == 'confirm' and not args.binaries:
        parser.error('fresh confirmation reuses frozen screen binaries')
    if args.mode == 'confirm' and not args.screen_admission:
        parser.error('confirmation requires explicit registered screen admission')
    base = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve()
    if not base.is_relative_to(Path('/run/media/kmosoti/data/FabricO11y/scratch')):
        raise RuntimeError('data-drive owned scratch required')
    out = args.out.resolve()
    if out.exists() or not out.is_relative_to(n.BASE/'query'):
        raise RuntimeError('fresh native-frontier query evidence path required')
    out.mkdir(parents=True)
    work = base/('key-first-'+out.name)
    work.mkdir()
    (work/'owned').write_text(str(out))
    (out/'objects').mkdir()
    (out/'binaries').mkdir()
    env = dict(os.environ,CARGO_BUILD_JOBS='2')
    for key in list(env):
        if key.startswith('FABRIC_') and key.endswith('_EXPERIMENT'):
            env.pop(key)
    env.update(FABRIC_BORROWED_LOG_EXPERIMENT='0',BENCH_PHASES='0')
    env.pop('BENCH_SHARED_CATALOG',None)
    env.pop('BENCH_DESCRIPTOR_REUSE',None)
    trials, identities, hashes = [], {}, {}
    start, status = time.monotonic(), 'failed'
    try:
        bounds(work,out)
        manifest = c.archive_sources(out)
        dump(out/'archive-controls.json',c.archive_controls())
        dump(out/'toolchain.json',dict(python=sys.version,
            rustc=subprocess.check_output(['rustc','-Vv'],cwd=ROOT,text=True),
            cargo=subprocess.check_output(['cargo','-V'],cwd=ROOT,text=True)))
        # Capture registered supplements that the generic source snapshot excludes.
        for name in ('native-frontier-protocol.md','native-key-first-proposal.md'):
            source = ROOT/'docs/experiments/benchmarks'/name
            raw = source.read_bytes()
            target = out/(name+'.gz')
            target.write_bytes(gzip.compress(raw,mtime=0))
            with target.open('rb') as stream:
                c.verify_source_gzip(stream,raw)
        if args.mode == 'controls':
            deadline = time.monotonic()+args.build_seconds+args.run_seconds
            outcomes = []
            controls = [('key_first',None),('completion_storage','missing_query_tables_are_incomplete_with_independently_declared_unavailability'),
                ('completion_storage','missing_raw_batches_preserve_projection_rows_but_mark_incomplete'),
                ('completion_storage','raw_batch_table_integrity_failures_are_incomplete')]
            for candidate in (False,True):
                for index,(suite,test) in enumerate(controls):
                    label = f'flag{int(candidate)}-control{index}'
                    argv = ['cargo','test','--offline','--locked','-p','fabric-server','--test',suite]
                    if test: argv += [test,'--','--exact','--nocapture']
                    else: argv += ['--','--nocapture']
                    native_env = dict(env,FABRIC_KEY_FIRST_EXPERIMENT=str(int(candidate)),FABRIC_SCRATCH_ROOT=str(work))
                    native_env['FABRIC_STORAGE_EVIDENCE'] = str(out/'storage-evidence'/label)
                    run(argv,native_env,out/(label+'.out'),out/(label+'.err'),deadline,work,out)
                    if suite == 'key_first':
                        values = []
                        for line in (out/(label+'.out')).read_text().splitlines():
                            try: value = json.loads(line)
                            except json.JSONDecodeError: continue
                            if isinstance(value,dict) and 'origin' in value and 'outcomes' in value:
                                values.append(value)
                        if len(values) != 1 or values[0]['origin']['key_first'] != candidate:
                            raise RuntimeError('native control compiled flag/receipt differs')
                        outcomes.append(values[0]['outcomes'])
            if len(outcomes) != 2 or outcomes[0] != outcomes[1]:
                raise RuntimeError('actual native corruption outcome differs between flags')
            for path in sorted(work.glob('key-first-corruption-*')):
                evidence = out/path.name
                evidence.mkdir()
                preserve_case(path,evidence,out/'objects',work,deadline)
                shutil.rmtree(path)
            c.check_source_identity(manifest)
            dump(out/'result.json',dict(mode='controls',native_test_commands=8,exit=0,
                borrowed=False,flags=[0,1],exact_corruption_outcomes=outcomes[0],
                control='actual History reader corruption and missing evidence'))
            status = 'passed'
            return
        build_deadline = time.monotonic()+args.build_seconds
        for counted in (False,True):
            for candidate in (False,True):
                name = ('counted' if counted else 'plain')+('-key-first' if candidate else '-legacy')
                if args.binaries:
                    source = args.binaries/'binaries'/(name+'.gz')
                    frozen = json.loads((args.binaries/'provenance.json').read_text())
                    expected = frozen['binary_sha256'][name]
                    shutil.copy2(source,out/'binaries'/(name+'.gz'))
                else:
                    argv = ['cargo','build','--offline','--locked','--release','-p','fabric-server','--example','key_first_query_probe']
                    if counted: argv += ['--features','responsibility-alloc-probe,phase-probe']
                    native_env = dict(env,FABRIC_KEY_FIRST_EXPERIMENT='1' if candidate else '0')
                    run(argv,native_env,out/(name+'-build.out'),out/(name+'-build.err'),build_deadline,work,out)
                    source = Path(os.environ['CARGO_TARGET_DIR'])/'release/examples/key_first_query_probe'
                    expected = c.grader.sha(source)
                    with source.open('rb') as src,gzip.open(out/'binaries'/(name+'.gz'),'wb') as dst:
                        shutil.copyfileobj(src,dst)
                    dump(out/(name+'-build-flags.json'),dict(argv=argv,FABRIC_KEY_FIRST_EXPERIMENT=native_env['FABRIC_KEY_FIRST_EXPERIMENT'],
                        FABRIC_BORROWED_LOG_EXPERIMENT='0',CARGO_BUILD_JOBS='2'))
                with gzip.open(out/'binaries'/(name+'.gz'),'rb') as stream:
                    if hashlib.file_digest(stream,'sha256').hexdigest() != expected:
                        raise RuntimeError('frozen binary decoded hash mismatch')
                hashes[name] = expected
        if args.binaries:
            admission = json.loads(args.screen_admission.read_text())
            if admission.get('decision') != 'confirm' or admission.get('screen_result_sha256') != c.grader.sha(args.binaries/'result.json'):
                raise RuntimeError('screen nomination/result authentication missing')
            dump(out/'screen-admission.json',admission)
            for name,receipt in frozen['source_manifest'].items():
                if name.endswith('.rs') or name.endswith('.toml') or name.endswith('.lock'):
                    if c.grader.sha(ROOT/name) != receipt['sha256']:
                        raise RuntimeError('confirmation Rust/build input differs from frozen screen')
        dump(out/'provenance.json',dict(revision=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
            binary_sha256=hashes,source_manifest=manifest,target=str(Path(os.environ['CARGO_TARGET_DIR'])),
            mode=args.mode,binaries_origin=str(args.binaries) if args.binaries else None))
        cells = ([(w,a,'sorted',42) for w in (16,1024) for a in (0,8)] if args.mode == 'screen'
                 else [(1024,8,order,seed) for seed in (43,44,45) for order in ('sorted','shuffled')])
        regimes = ('counted-on','counted-off','plain') if args.mode == 'screen' else ('counted-off','plain')
        deadline = time.monotonic()+args.run_seconds
        for ci,(width,attrs,order,seed) in enumerate(cells):
            for regime in regimes:
                for candidate in ((False,True) if (ci+regimes.index(regime))%2 == 0 else (True,False)):
                    variant = ('plain' if regime == 'plain' else 'counted')+('-key-first' if candidate else '-legacy')
                    label = f'w{width}-a{attrs}-{order}-s{seed}-{regime}-{variant}'
                    evidence = out/label
                    evidence.mkdir()
                    trial = work/label
                    binary = work/'active-binary'
                    with gzip.open(out/'binaries'/(variant+'.gz'),'rb') as src,binary.open('wb') as dst:
                        shutil.copyfileobj(src,dst)
                    if c.grader.sha(binary) != hashes[variant]:
                        raise RuntimeError('active binary hash differs')
                    binary.chmod(0o700)
                    native_env = dict(env,BENCH_PHASES='1' if regime == 'counted-on' else '0')
                    argv = ['/usr/bin/time','-o',str(evidence/'process.json'),'-f',
                        '{"wall_seconds":%e,"user_cpu_seconds":%U,"system_cpu_seconds":%S,"max_rss_kib":%M,"exit":%x}',
                        str(binary),str(trial),str(width),str(attrs),order,str(seed)]
                    run(argv,native_env,evidence/'native.out',evidence/'native.err',deadline,work,out,raw=trial)
                    summary = grade(trial,evidence,out/'objects')
                    f = summary['fixture']
                    if (f['key_first'] != candidate or f['counted'] != (regime != 'plain') or f['borrowed']
                            or f['phase_on'] != (regime == 'counted-on') or
                            (f['records'],f['width'],f['attrs'],f['order'],f['seed'],f['limit'],f['timestamp_ties'])
                            != (2048,width,attrs,order,seed,64,4)):
                        raise RuntimeError('compiled flag/fixture identity differs')
                    identity = width,attrs,order,seed
                    if identity in identities and identities[identity] != summary['source_sha256']:
                        raise RuntimeError('matched arms/regimes have different exact source bytes')
                    identities[identity] = summary['source_sha256']
                    preserve_case(trial,evidence,out/'objects',work,deadline)
                    trials.append(dict(label=label,regime=regime,candidate=candidate,**summary))
                    dump(out/'trials.json',trials)
                    c.check_source_identity(manifest)
                    bounds(work,out)
                    binary.unlink()
                    shutil.rmtree(trial)
        if len(trials) != 24:
            raise RuntimeError('24 matched children required per registered stage')
        paired = []
        for trial in trials:
            if not trial['candidate']: continue
            f = trial['fixture']
            before = next(t for t in trials if not t['candidate'] and t['regime'] == trial['regime'] and
                all(t['fixture'][k] == f[k] for k in ('width','attrs','order','seed')))
            for population,value in trial['metrics'].items():
                baseline = before['metrics'][population]
                ratios = {k:value[k]/baseline[k] if baseline[k] else None for k in value}
                paired.append(dict(fixture=f,regime=trial['regime'],population=population,
                    baseline=baseline,candidate=value,ratios=ratios))
        dump(out/'result.json',dict(trials=trials,paired=paired,canonical_chains=576,
            measured_first_pages=2304,canonical_pages=sum(t['canonical_pages'] for t in trials),
            negative_controls=sum(t['negative_controls'] for t in trials),mode=args.mode,
            limitations=['Canonical full drains are correctness evidence, outside first-page timings.',
                'First/reuse are History boundaries, not OS cold/warm caches.',
                'Native sealing sorts full keys; shuffled input is not mixed Parquet groups.',
                'Counted/phase timing is observational; plain timing decides CPU benefit.']))
        status = 'passed'
    except BaseException as error:
        dump(out/'failure.json',dict(error=repr(error),scratch=str(work),status='failed',
            full_state_preserved=True))
        raise
    finally:
        accounting_error = None
        pending_exception = sys.exc_info()[0] is not None
        try: size = n.footprint(work)
        except (OSError,RuntimeError) as error:
            size,accounting_error = None,repr(error)
            if status == 'passed': status = 'failed'
        if status == 'passed':
            if (work/'owned').read_text() != str(out): raise RuntimeError('scratch ownership differs')
            shutil.rmtree(work)
        try: retained = n.footprint(out)
        except (OSError,RuntimeError) as error:
            retained = None
            accounting_error = accounting_error or repr(error)
            status = 'failed'
        dump(out/'cleanup.json',dict(status=status,removed=not work.exists(),scratch=str(work),
            scratch_bytes=size,accounting_error=accounting_error,retained_bytes=retained,
            elapsed_seconds=time.monotonic()-start))
        if accounting_error and not pending_exception:
            raise RuntimeError('cleanup accounting failed: '+accounting_error)


if __name__ == '__main__': main()
