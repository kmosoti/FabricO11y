#!/usr/bin/env python3
"""Run registered cost cells into a fresh, preserved directory. Smoke is never formal evidence."""
import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
STORAGE = ROOT / 'tools/storage-probe/target/release'
LAYOUT = ROOT / 'tools/layout-probe/target/release'

def sha(path): return hashlib.sha256(path.read_bytes()).hexdigest()
def run(cmd, directory, ledger):
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        process = subprocess.Popen([str(v) for v in cmd], cwd=ROOT, stdout=stdout, stderr=stderr)
        _, status, usage = os.wait4(process.pid, 0)
        process.returncode = os.waitstatus_to_exitcode(status)
        stdout.seek(0); stderr.seek(0)
        entry = {'command': list(map(str, cmd)), 'exit_code': process.returncode,
                 'stdout': stdout.read().decode(errors='replace'), 'stderr': stderr.read().decode(errors='replace'),
                 'whole_process_user_ns':round(usage.ru_utime*1_000_000_000),
                 'whole_process_system_ns':round(usage.ru_stime*1_000_000_000),
                 'whole_process_cpu_ns':round((usage.ru_utime+usage.ru_stime)*1_000_000_000),
                 'whole_process_max_rss_kib':usage.ru_maxrss}
    ledger.append(entry)
    (directory / f'command-{len(ledger):05}.json').write_text(json.dumps(entry, indent=2) + '\n')
    if process.returncode: raise RuntimeError(f'child failed, exit {process.returncode}: {cmd}')
    return entry

def nonnegative(value, label, positive=False):
    if type(value) is not int or value < (1 if positive else 0):
        raise RuntimeError(f'invalid {label}: {value!r}')
    return value

def nearest_rank(values, percentile):
    if not values: raise RuntimeError('empty rank input')
    return sorted(values)[(len(values)*percentile+99)//100-1]

def measure(value, label, positive_wall=False, positive_cpu=False):
    if not isinstance(value, dict): raise RuntimeError(f'missing measure: {label}')
    nonnegative(value.get('wall_ns'), f'{label}.wall_ns', positive_wall)
    nonnegative(value.get('cpu_ns'), f'{label}.cpu_ns', positive_cpu)

def checked(path, bench, mode, shape, seed, trial, expected_samples):
    if not path.is_file(): raise RuntimeError(f'missing result: {path}')
    data = json.loads(path.read_text())
    for field, wanted in [('schema',1),('benchmark',bench),('mode',mode),('shape',shape),('seed',seed),('trial',trial),('checks_ok',True)]:
        if data.get(field) != wanted: raise RuntimeError(f'invalid {path}: {field}={data.get(field)!r}, wanted {wanted!r}')
    nonnegative(data.get('process_cpu_ns'), 'process_cpu_ns')
    nonnegative(data.get('rss_kib'), 'rss_kib')
    kernel_io=data.get('kernel_io_delta')
    if not isinstance(kernel_io,dict): raise RuntimeError('missing kernel I/O counters')
    for field in ('rchar','wchar','read_bytes','write_bytes'): nonnegative(kernel_io.get(field),f'kernel_io_delta.{field}')
    if not isinstance(data.get('calibration'),list) or len(data['calibration'])!=100: raise RuntimeError(f'missing timer calibration: {path}')
    for index, timer in enumerate(data['calibration']): measure(timer, f'calibration[{index}]')
    if bench == 'sidecar':
        if data.get('query_count') != (128 if data.get('role') == 'server' else 0): raise RuntimeError('invalid sidecar query count')
        measure(data.get('phase'), 'sidecar phase', True, True)
        nonnegative(data.get('hint_bytes'), 'hint_bytes')
        nonnegative(data.get('source_bytes'), 'source_bytes', True)
        if data.get('role') == 'server' and data.get('fallback') not in (True, False): raise RuntimeError('missing fallback status')
    if expected_samples is not None:
        events = nonnegative(data.get('events'), 'events', True)
        query_count = 512 if bench == 'layout' and events == 8192 else 128
        if data.get('query_count') != query_count: raise RuntimeError(f'invalid query count: {path}')
        variants = ('scalar','receipt') if bench == 'receipt' else (('full','projected') if data.get('layout') == 'json64' else ('full','projected','postings'))
        if bench == 'layout' and data.get('layout') not in ('json64','plain64','zstd64','zstd256'): raise RuntimeError('invalid layout')
        samples = data.get('samples')
        if not isinstance(samples,list) or len(samples)!=expected_samples or expected_samples!=query_count*len(variants): raise RuntimeError(f'invalid sample count: {path}')
        grid = {}
        for sample in samples:
            if not isinstance(sample,dict): raise RuntimeError('invalid sample object')
            index=nonnegative(sample.get('index'),'sample index')
            if index>=query_count or sample.get('family') != index%8 or type(sample.get('family')) is not int: raise RuntimeError('invalid sample index/family')
            variant=sample.get('mode')
            if variant not in variants or (variant,index) in grid: raise RuntimeError('duplicate or invalid sample mode/index')
            measure(sample.get('timing'),f'{variant}[{index}]',True)
            matches=nonnegative(sample.get('matches'),'sample matches')
            if matches>events: raise RuntimeError('sample matches exceed events')
            if bench=='layout' and nonnegative(sample.get('logical_data_bytes'),'logical_data_bytes') != data.get('layout_bytes'): raise RuntimeError('invalid logical data bytes')
            if bench=='receipt':
                if variant=='receipt': nonnegative(sample.get('page_bytes'),'receipt page bytes',True)
                elif sample.get('page_bytes') is not None: raise RuntimeError('scalar sample has page bytes')
            grid[(variant,index)]=sample
        if set(grid)!={(variant,index) for variant in variants for index in range(query_count)}: raise RuntimeError('missing sample mode/index')
        for index in range(query_count):
            if len({grid[(variant,index)]['matches'] for variant in variants})!=1: raise RuntimeError('wrong matched count across variants')
        for field, variant, pct in ((('p50_scalar_ns','scalar',50),('p99_scalar_ns','scalar',99),('p50_receipt_ns','receipt',50),('p99_receipt_ns','receipt',99)) if bench=='receipt' else (('p50_full_ns','full',50),('p99_full_ns','full',99),('p50_projected_ns','projected',50),('p99_projected_ns','projected',99))):
            raw=[grid[(variant,index)]['timing']['wall_ns'] for index in range(query_count)]
            if data.get(field)!=nearest_rank(raw,pct): raise RuntimeError(f'derived {field} differs from raw samples')
        if bench=='layout':
            for field in ('layout_bytes','metadata_bytes','parquet_footer_bytes','table_anchor_bytes','index_bytes','index_data_bytes','index_digest_bytes'): nonnegative(data.get(field),field)
            nonnegative(data['layout_bytes'],'layout_bytes',True)
            nonnegative(data['metadata_bytes'],'metadata_bytes',True)
            if data['index_bytes']!=data['index_data_bytes']+data['index_digest_bytes']: raise RuntimeError('index byte total mismatch')
            if data['layout']=='json64':
                if data['index_bytes']!=0 or data['table_anchor_bytes']!=0 or data['index_build'] is not None or data['index_publication'] is not None: raise RuntimeError('JSON has unexpected index/anchor')
            else:
                if data['metadata_bytes']!=data['table_anchor_bytes'] or data['table_anchor_bytes']==0 or data['index_digest_bytes']!=64: raise RuntimeError('missing persisted trust bytes')
                nonnegative(data['parquet_footer_bytes'],'parquet_footer_bytes',True)
                nonnegative(data['index_data_bytes'],'index_data_bytes',True)
                measure(data.get('index_build'),'index build',True)
                measure(data.get('index_publication'),'index publication',True)
            measure(data.get('publication'),'table publication',True)
            measure(data.get('file_read'),'file read',True)
        else:
            for field in ('scalar_build','sealed_build','verify','partial_resume_merge','replay'): measure(data.get(field),field,True)
            for field in ('full_page_bytes','two_page_bytes','residual_bytes'): nonnegative(data.get(field),field,True)
    return data

def validate_pair(source_digest, results):
    """Reject mutated source or answer bindings across independent cost cells."""
    if not results or any(row.get('source_sha256') != source_digest for row in results):
        raise RuntimeError('source hash mismatch across methods')
    hashes = [row.get('answer_sha256') for row in results]
    if any(not isinstance(value, str) or len(value) != 64 for value in hashes) or len(set(hashes)) != 1:
        raise RuntimeError('answer hash mismatch across methods')

def validate_layout_artifacts(directory, row):
    """Bind reported bytes to every persisted table and external trust file."""
    if row['layout']=='json64':
        meta=(directory/'metadata.json').read_bytes()
        if len(meta)!=row['metadata_bytes']: raise RuntimeError('JSON metadata byte mismatch')
        hashes=json.loads(meta)
        if not isinstance(hashes,list) or len(hashes)!=(row['events']+63)//64: raise RuntimeError('JSON block hash count mismatch')
        blocks=[(directory/f'block-{i:04}.json').read_bytes() for i in range(len(hashes))]
        if any(hashlib.sha256(blob).hexdigest()!=digest for blob,digest in zip(blocks,hashes)):
            raise RuntimeError('JSON block hash mismatch')
        if sum(map(len,blocks))!=row['layout_bytes'] or hashlib.sha256(''.join(hashes).encode()).hexdigest()!=row['output_sha256']:
            raise RuntimeError('JSON output byte/hash mismatch')
    else:
        table=(directory/'table.parquet').read_bytes()
        anchor=(directory/'table-anchor.json').read_bytes()
        postings=(directory/'postings.json').read_bytes()
        digest=(directory/'postings-digest.txt').read_bytes()
        if len(table)!=row['layout_bytes'] or hashlib.sha256(table).hexdigest()!=row['output_sha256']:
            raise RuntimeError('Parquet table byte/hash mismatch')
        if len(table)<8 or table[-4:]!=b'PAR1' or int.from_bytes(table[-8:-4],'little')!=row['parquet_footer_bytes']:
            raise RuntimeError('Parquet footer byte mismatch')
        if len(anchor)!=row['table_anchor_bytes'] or len(anchor)!=row['metadata_bytes']:
            raise RuntimeError('Parquet anchor byte mismatch')
        parsed=json.loads(anchor)
        if parsed.get('sha256')!=list(hashlib.sha256(table).digest()) or parsed.get('rows')!=row['events'] or parsed.get('version')!=1:
            raise RuntimeError('Parquet anchor binding mismatch')
        if len(postings)!=row['index_data_bytes'] or len(digest)!=row['index_digest_bytes'] or len(postings)+len(digest)!=row['index_bytes']:
            raise RuntimeError('index byte mismatch')
        if digest.decode('ascii')!=hashlib.sha256(postings).hexdigest():
            raise RuntimeError('index digest mismatch')

def source_hashes():
    paths = [ROOT / p for p in ('Cargo.toml','Cargo.lock','tools/storage-probe/Cargo.toml','tools/storage-probe/Cargo.lock','tools/layout-probe/Cargo.toml','tools/layout-probe/Cargo.lock')]
    for directory in ('src','tools/storage-probe/src','tools/layout-probe/src'):
        paths += list((ROOT/directory).rglob('*.rs'))
    paths += list((ROOT/'docs/experiments/benchmarks').glob('*protocol.md'))
    paths.append(Path(__file__))
    paths.append(ROOT / 'tools/bench/test_research_costs.py')
    return {str(p.relative_to(ROOT)):sha(p) for p in sorted(set(paths))}

def host_load_snapshot():
    return {'loadavg_1_5_15':os.getloadavg(),
            'processes_pid_comm_etime_pcpu':subprocess.check_output(
                ['ps','-eo','pid,comm,etime,pcpu','--no-headers'],text=True).splitlines()}

def summarize(output, combos, trials):
    """Retain paired decisions by dataset; exclude the registered warmup."""
    def validated(path, bench, shape, seed, trial, samples):
        candidate=json.loads(path.read_text())
        if candidate.get('mode') not in ('smoke','formal'): raise RuntimeError(f'invalid result mode: {path}')
        return checked(path,bench,candidate['mode'],shape,seed,trial,samples)
    def raw_rank(row, variant, percentile):
        return nearest_rank([s['timing']['wall_ns'] for s in row['samples'] if s['mode']==variant],percentile)
    report = {}
    for shape, seed, count in combos:
        key = f'{shape}-{seed}-{count}'
        entries = [t for t in trials if t >= 0]
        layout_rows = {}
        for layout in ('json64','plain64','zstd64','zstd256'):
            layout_rows[layout] = [validated(output/f'{key}-{t}-{layout}/result.json','layout',shape,seed,t,(2 if layout=='json64' else 3)*(512 if count==8192 else 128)) for t in entries]
        json_bytes = statistics.median(r['layout_bytes']+r['metadata_bytes'] for r in layout_rows['json64'])
        json_projected = statistics.median(raw_rank(r,'projected',50) for r in layout_rows['json64'])
        layouts = {}
        for layout, rows in layout_rows.items():
            byte_median = statistics.median(r['layout_bytes']+r['metadata_bytes'] for r in rows)
            with_index = statistics.median(r['layout_bytes']+r['metadata_bytes']+r['index_bytes'] for r in rows)
            projected_median = statistics.median(raw_rank(r,'projected',50) for r in rows)
            rare_savings = []
            for row in rows:
                proj = [s['timing']['wall_ns'] for s in row['samples'] if s['mode']=='projected' and s['family']==3]
                post = [s['timing']['wall_ns'] for s in row['samples'] if s['mode']=='postings' and s['family']==3]
                if post: rare_savings.append(statistics.median(proj)-statistics.median(post))
            saving = statistics.median(rare_savings) if rare_savings else None
            build = statistics.median(r['index_build']['wall_ns'] for r in rows) if saving is not None else None
            publication = statistics.median(r['index_publication']['wall_ns'] for r in rows) if saving is not None else None
            combined = statistics.median(r['index_build']['wall_ns']+r['index_publication']['wall_ns'] for r in rows) if saving is not None else None
            layouts[layout] = {'median_bytes':byte_median,'median_bytes_with_index':with_index,'median_projected_query_ns':projected_median,
                'byte_ratio_to_json':byte_median/json_bytes,'projected_ratio_to_json':projected_median/json_projected,
                'registered_layout_gate':byte_median<=0.8*json_bytes and projected_median<=1.1*json_projected if layout!='json64' else None,
                'rare_postings_median_saving_ns':saving,'postings_build_median_ns':build,
                'postings_publication_median_ns':publication,'postings_build_plus_publication_median_ns':combined,
                'measured_break_even_queries':int(-(-combined//saving)) if saving is not None and saving>0 else None}
        cell = {'layouts':layouts}
        if count==2048:
            receipt_trials=[validated(output/f'{key}-{t}-receipt.json','receipt',shape,seed,t,256) for t in entries]
            receipt_pairs=[]
            for row in receipt_trials:
                scalar={s['index']:s for s in row['samples'] if s['mode']=='scalar'}
                sealed={s['index']:s for s in row['samples'] if s['mode']=='receipt'}
                for i in range(128):
                    receipt_pairs.append({'trial':row['trial'],'query':i,'family':i%8,
                        'scalar_wall_ns':scalar[i]['timing']['wall_ns'],'receipt_wall_ns':sealed[i]['timing']['wall_ns'],
                        'ratio':sealed[i]['timing']['wall_ns']/scalar[i]['timing']['wall_ns'],
                        'receipt_page_bytes':sealed[i]['page_bytes']})
            cell['receipt']={'paired':receipt_pairs,'median_ratio_by_family':{
                str(f):statistics.median(p['ratio'] for p in receipt_pairs if p['family']==f) for f in range(8)},
                'p99_ratio_by_family':{str(f):sorted(p['ratio'] for p in receipt_pairs if p['family']==f)[-1] for f in range(8)}}
            pairs=[]
            for t in entries:
                prefix=f'{key}-{t}'
                source=validated(output/f'{prefix}-source-cost.json','sidecar',shape,seed,t,None)
                baseline=validated(output/f'{prefix}-sidecar-baseline.json','sidecar',shape,seed,t,None)
                valid=validated(output/f'{prefix}-sidecar-valid.json','sidecar',shape,seed,t,None)
                pairs.append({'trial':t,'baseline_server_cpu_ns':baseline['phase']['cpu_ns'],
                    'sidecar_source_cpu_ns':source['phase']['cpu_ns'],'sidecar_server_cpu_ns':valid['phase']['cpu_ns'],
                    'hint_bytes':source['hint_bytes'], 'source_bytes':source['source_bytes'],
                    'server_cpu_ratio':valid['phase']['cpu_ns']/baseline['phase']['cpu_ns'],
                    'total_cpu_ratio':(source['phase']['cpu_ns']+valid['phase']['cpu_ns'])/baseline['phase']['cpu_ns']})
            cell['sidecar']={'paired':pairs,'median_server_cpu_ratio':statistics.median(p['server_cpu_ratio'] for p in pairs),
                'median_total_cpu_ratio':statistics.median(p['total_cpu_ratio'] for p in pairs),
                'registered_server_gate':statistics.median(p['server_cpu_ratio'] for p in pairs)<=0.9,
                'registered_total_gate':statistics.median(p['total_cpu_ratio'] for p in pairs)<=1.1}
        report[key]=cell
    return report

def main():
    parser=argparse.ArgumentParser(); parser.add_argument('output',type=Path); group=parser.add_mutually_exclusive_group(required=True)
    group.add_argument('--smoke',action='store_true'); group.add_argument('--formal',action='store_true')
    args=parser.parse_args(); output=args.output.resolve()
    if os.path.lexists(output): raise RuntimeError(f'output path already exists: {output}')
    output.mkdir(parents=True); ledger=[]
    filesystem=subprocess.check_output(['findmnt','--target',str(output),'--output','SOURCE,FSTYPE,TARGET','--noheadings'],text=True).strip()
    if args.formal and ' tmpfs ' in f' {filesystem} ': raise RuntimeError('formal publication measurements require a durable filesystem; output is tmpfs')
    meta={'mode':'smoke' if args.smoke else 'formal','started_utc':dt.datetime.now(dt.timezone.utc).isoformat(),
          'platform':platform.platform(),'python':sys.version,'output_filesystem':filesystem,'git_head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
          'git_status':subprocess.check_output(['git','status','--short'],cwd=ROOT,text=True),'source_sha256':source_hashes(),
          'active_work_note':'shared host; start/end process lists and load averages are observations, not host isolation; no OS cache drop',
          'host_load_start':host_load_snapshot(),
          'cpu_model':next((line.split(':',1)[1].strip() for line in Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')),None)}
    (output/'metadata.json').write_text(json.dumps(meta,indent=2)+'\n')
    try:
        for manifest,bins in [('tools/storage-probe/Cargo.toml',['receipt-cost','sidecar-cost']),('tools/layout-probe/Cargo.toml',['layout-cost'])]:
            cmd=['cargo','build','--offline','--locked','--release','--manifest-path',manifest]
            for binary in bins:cmd += ['--bin',binary]
            run(cmd,output,ledger)
        combos=[('mixed',201,2048)] if args.smoke else [(shape,seed,2048) for shape in ('mixed','shuffled_logs') for seed in (201,202,203)] + [(shape,201,8192) for shape in ('mixed','shuffled_logs')]
        trials=[-1,0] if args.smoke else list(range(-1,5)) # -1 is the registered warmup
        for shape,seed,count in combos:
            source=output/f'{shape}-{seed}-{count}-source.json'
            run([STORAGE/'receipt-cost','fixture',shape,seed,count,source],output,ledger)
            source_digest=sha(source)
            for trial in trials:
                name=f'{shape}-{seed}-{count}-{trial}'
                pair_results=[]
                if count==2048:
                    receipt=output/f'{name}-receipt.json'
                    run([STORAGE/'receipt-cost',meta['mode'],shape,seed,trial,source,receipt],output,ledger)
                    data=checked(receipt,'receipt',meta['mode'],shape,seed,trial,256)
                    if data['source_sha256']!=source_digest:raise RuntimeError('receipt source hash mismatch')
                    pair_results.append(data)
                    hint=output/f'{name}-hint.json'; src=output/f'{name}-source-cost.json'
                    run([STORAGE/'sidecar-cost','source',meta['mode'],shape,seed,trial,source,hint,src,'valid'],output,ledger)
                    s=checked(src,'sidecar',meta['mode'],shape,seed,trial,None)
                    if s['role']!='source' or s['query_count']!=0 or s['source_sha256']!=source_digest or not hint.is_file() or s['hint_bytes']!=hint.stat().st_size:raise RuntimeError('invalid sidecar source result')
                    order=['baseline','valid'] if trial%2==0 else ['valid','baseline']
                    roots=[]
                    for scenario in order:
                        dest=output/f'{name}-sidecar-{scenario}.json'
                        run([STORAGE/'sidecar-cost','server',meta['mode'],shape,seed,trial,source,hint,dest,scenario],output,ledger)
                        result=checked(dest,'sidecar',meta['mode'],shape,seed,trial,None)
                        if result['role']!='server' or result['query_count']!=128 or result['source_sha256']!=source_digest:raise RuntimeError('invalid sidecar server result')
                        if result['hint_bytes'] != (0 if scenario=='baseline' else hint.stat().st_size):raise RuntimeError('incorrect sidecar hint bytes')
                        roots.append(result['anchor_root'])
                    if roots[0]!=roots[1]:raise RuntimeError('sidecar root mismatch')
                    for scenario in ('absent','corrupt','stale','underinclusive'):
                        bad=output/f'{name}-{scenario}-hint.json'
                        if scenario=='corrupt':bad.write_text('{broken')
                        elif scenario=='stale':bad.write_text(json.dumps({'source_sha256':'0'*64,'summaries':[]}))
                        elif scenario=='underinclusive':
                            payload=json.loads(hint.read_text());payload['summaries'][0]['tokens']=[];bad.write_text(json.dumps(payload))
                        dest=output/f'{name}-sidecar-{scenario}.json'
                        run([STORAGE/'sidecar-cost','server',meta['mode'],shape,seed,trial,source,bad,dest,scenario],output,ledger)
                        neg=checked(dest,'sidecar',meta['mode'],shape,seed,trial,None)
                        if neg.get('fallback') is not True or neg.get('anchor_root')!=roots[0]:raise RuntimeError('negative hint failed to fall back')
                        if neg['hint_bytes'] != (0 if scenario=='absent' else bad.stat().st_size):raise RuntimeError('incorrect negative hint bytes')
                layouts=['json64','plain64','zstd64','zstd256']
                if trial%2:layouts.reverse()
                for layout in layouts:
                    dest=output/f'{name}-{layout}'
                    run([LAYOUT/'layout-cost',meta['mode'],shape,seed,count,trial,layout,source,dest],output,ledger)
                    result=checked(dest/'result.json','layout',meta['mode'],shape,seed,trial,(2 if layout=='json64' else 3)*(512 if count==8192 else 128))
                    validate_layout_artifacts(dest,result)
                    if result['source_sha256']!=source_digest or result['layout']!=layout:raise RuntimeError('layout source/config mismatch')
                    pair_results.append(result)
                validate_pair(source_digest,pair_results)
        (output/'summary.json').write_text(json.dumps(summarize(output,combos,trials),indent=2)+'\n')
        meta['completed_utc']=dt.datetime.now(dt.timezone.utc).isoformat();meta['status']='complete'
    except Exception as exc:
        meta['status']='failed';meta['error']=str(exc);raise
    finally:
        meta['host_load_end']=host_load_snapshot()
        meta['commands']=ledger
        (output/'metadata.json').write_text(json.dumps(meta,indent=2)+'\n')
        hashes={str(p.relative_to(output)):sha(p) for p in output.rglob('*') if p.is_file() and p.name!='SHA256SUMS.json'}
        (output/'SHA256SUMS.json').write_text(json.dumps(hashes,indent=2,sort_keys=True)+'\n')
if __name__=='__main__':
    try:main()
    except Exception as e:print(e,file=sys.stderr);sys.exit(1)
