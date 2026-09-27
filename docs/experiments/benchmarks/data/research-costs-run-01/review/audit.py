#!/usr/bin/env python3
"""Independent read-only audit of the registered research-cost result files."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import statistics


def need(ok, message):
    if not ok:
        raise AssertionError(message)


def load(path):
    return json.loads(path.read_text())


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def rank(values, pct):
    need(bool(values), 'empty rank input')
    return sorted(values)[math.ceil(len(values) * pct / 100) - 1]


def equal(actual, expected, label):
    if isinstance(expected, float):
        need(isinstance(actual, (int, float)) and math.isclose(actual, expected, rel_tol=1e-12), f'{label}: {actual!r} != {expected!r}')
    else:
        need(actual == expected, f'{label}: {actual!r} != {expected!r}')


def samples(row, modes, queries, label):
    grid = {}
    for item in row['samples']:
        mode, index = item['mode'], item['index']
        need(type(index) is int and 0 <= index < queries, f'{label}: bad query index')
        need(type(item['family']) is int and item['family'] == index % 8, f'{label}: wrong family')
        need(mode in modes and (mode, index) not in grid, f'{label}: duplicate/unknown sample')
        need(type(item['matches']) is int and 0 <= item['matches'] <= row['events'], f'{label}: wrong match count')
        for timer in ('wall_ns', 'cpu_ns'):
            need(type(item['timing'][timer]) is int and item['timing'][timer] >= 0, f'{label}: invalid timer')
        need(item['timing']['wall_ns'] > 0, f'{label}: zero wall timer')
        grid[(mode, index)] = item
    need(set(grid) == {(mode, i) for mode in modes for i in range(queries)}, f'{label}: incomplete sample grid')
    for i in range(queries):
        need(len({grid[(mode, i)]['matches'] for mode in modes}) == 1, f'{label}: paired match counts differ at {i}')
    return grid


def audit_layout(directory, source_hash, answer_hash, count, trial, mode):
    row = load(directory / 'result.json')
    name = directory.name
    layout = row['layout']
    need(row['mode'] == mode and row['trial'] == trial and row['events'] == count and row['query_count'] == (512 if count == 8192 else 128), f'{name}: wrong identity')
    need(row['checks_ok'] is True and row['source_sha256'] == source_hash and row['answer_sha256'] == answer_hash, f'{name}: source/answer binding')
    n = row['query_count']
    variants = ('full', 'projected') if layout == 'json64' else ('full', 'projected', 'postings')
    grid = samples(row, variants, n, name)
    for field, variant, pct in [('p50_full_ns','full',50),('p99_full_ns','full',99),('p50_projected_ns','projected',50),('p99_projected_ns','projected',99)]:
        equal(row[field], rank([grid[(variant,i)]['timing']['wall_ns'] for i in range(n)],pct), name+' '+field)
    if layout == 'json64':
        meta = (directory/'metadata.json').read_bytes()
        hashes = json.loads(meta)
        need(len(meta) == row['metadata_bytes'] and len(hashes) == math.ceil(count/64), f'{name}: metadata bytes')
        blocks = [(directory/f'block-{i:04}.json').read_bytes() for i in range(len(hashes))]
        need(all(hashlib.sha256(b).hexdigest() == h for b,h in zip(blocks,hashes)), f'{name}: JSON block digest')
        need(sum(map(len,blocks)) == row['layout_bytes'], f'{name}: JSON data bytes')
        need(hashlib.sha256(''.join(hashes).encode()).hexdigest() == row['output_sha256'], f'{name}: JSON output digest')
        need(row['index_bytes'] == 0 and row['table_anchor_bytes'] == 0, f'{name}: unexpected trust bytes')
    else:
        table = (directory/'table.parquet').read_bytes()
        anchor = (directory/'table-anchor.json').read_bytes()
        postings = (directory/'postings.json').read_bytes()
        external = (directory/'postings-digest.txt').read_bytes()
        need(len(table) == row['layout_bytes'] and hashlib.sha256(table).hexdigest() == row['output_sha256'], f'{name}: table size/digest')
        need(table[-4:] == b'PAR1' and int.from_bytes(table[-8:-4], 'little') == row['parquet_footer_bytes'], f'{name}: Parquet footer')
        need(len(anchor) == row['table_anchor_bytes'] == row['metadata_bytes'], f'{name}: anchor bytes')
        binding = json.loads(anchor)
        need(binding == {'sha256': list(hashlib.sha256(table).digest()), 'rows': count, 'version': 1}, f'{name}: anchor binding')
        need(len(postings) == row['index_data_bytes'] and len(external) == row['index_digest_bytes'] == 64, f'{name}: index sizes')
        need(row['index_bytes'] == len(postings)+len(external) and external.decode('ascii') == hashlib.sha256(postings).hexdigest(), f'{name}: index binding')
        need(row['index_build']['wall_ns'] > 0 and row['index_publication']['wall_ns'] > 0, f'{name}: index phases')
    return row, grid


def audit_receipt(path, source_hash, answer_hash, trial, mode):
    row = load(path)
    need(row['mode'] == mode and row['trial'] == trial and row['events'] == 2048 and row['query_count'] == 128, f'{path}: identity')
    need(row['checks_ok'] is True and row['source_sha256'] == source_hash and row['answer_sha256'] == answer_hash, f'{path}: binding')
    grid = samples(row, ('scalar','receipt'),128,str(path))
    for field, variant, pct in [('p50_scalar_ns','scalar',50),('p99_scalar_ns','scalar',99),('p50_receipt_ns','receipt',50),('p99_receipt_ns','receipt',99)]:
        equal(row[field],rank([grid[(variant,i)]['timing']['wall_ns'] for i in range(128)],pct),str(path)+' '+field)
    equal(row['full_page_bytes'],sum(grid[('receipt',i)]['page_bytes'] for i in range(128)),str(path)+' full page bytes')
    return row, grid


def audit_sidecar(root, key, trial, source_hash, answer_hash, mode):
    p = f'{key}-{trial}'
    role_names = ('source-cost','sidecar-baseline','sidecar-valid','sidecar-absent','sidecar-corrupt','sidecar-stale','sidecar-underinclusive')
    rows = {name:load(root/f'{p}-{name}.json') for name in role_names}
    source_size = (root/f'{key}-source.json').stat().st_size
    hint_size = (root/f'{p}-hint.json').stat().st_size
    root_hash = rows['sidecar-baseline']['anchor_root']
    for name,row in rows.items():
        need(row['mode'] == mode and row['trial'] == trial and row['checks_ok'] is True, f'{p}-{name}: identity')
        need(row['source_sha256'] == source_hash and row['source_bytes'] == source_size, f'{p}-{name}: source binding')
        need(row['phase']['wall_ns'] > 0 and row['phase']['cpu_ns'] > 0, f'{p}-{name}: phase timer')
        if name == 'source-cost':
            need(row['role'] == 'source' and row['query_count'] == 0 and row['hint_bytes'] == hint_size, f'{p}: source hint')
        else:
            scenario = name.removeprefix('sidecar-')
            expected = hint_size if scenario == 'valid' else 0 if scenario in ('baseline','absent') else (root/f'{p}-{scenario}-hint.json').stat().st_size
            need(row['role'] == 'server' and row['query_count'] == 128 and row['hint_bytes'] == expected, f'{p}-{name}: hint bytes')
            need(row['anchor_root'] == root_hash and row['fallback'] is (scenario != 'valid'), f'{p}-{name}: root/fallback')
    return rows


def family_stats(rows, variant):
    return {str(f):{'p50_ns':rank([s['timing']['wall_ns'] for r in rows for s in r['samples'] if s['mode']==variant and s['family']==f],50),
                     'p99_ns':rank([s['timing']['wall_ns'] for r in rows for s in r['samples'] if s['mode']==variant and s['family']==f],99)} for f in range(8)}


def audit(root, mode):
    meta = load(root/'metadata.json')
    need(meta['status'] == 'complete', f'run is {meta["status"]}; wait for completion')
    need(all(c['exit_code'] == 0 for c in meta['commands']), 'child command failed')
    need('host_load_start' in meta and 'host_load_end' in meta, 'missing load snapshots')
    manifest = load(root/'SHA256SUMS.json')
    files = {str(path.relative_to(root)):path for path in root.rglob('*') if path.is_file() and path.name != 'SHA256SUMS.json'}
    need(set(files) == set(manifest), 'SHA256SUMS file set differs from retained files')
    for name,path in files.items():
        need(digest(path) == manifest[name], f'preserved hash mismatch: {name}')
    repo = Path(__file__).resolve().parents[2]
    six = ('tools/storage-probe/src/bin/common/mod.rs','tools/storage-probe/src/bin/receipt-cost.rs',
           'tools/storage-probe/src/bin/sidecar-cost.rs','tools/layout-probe/src/bin/layout-cost.rs',
           'tools/bench/run_research_costs.py','tools/bench/test_research_costs.py')
    source_code_hashes = {}
    for name in six:
        source_code_hashes[name] = digest(repo/name)
        need(source_code_hashes[name] == meta['source_sha256'][name], f'code hash differs from run metadata: {name}')
    trials = [0] if mode == 'smoke' else list(range(5))
    all_trials = [-1]+trials
    combos = [('mixed',201,2048)] if mode == 'smoke' else [(shape,seed,2048) for shape in ('mixed','shuffled_logs') for seed in (201,202,203)] + [(shape,201,8192) for shape in ('mixed','shuffled_logs')]
    summary = load(root/'summary.json')
    result = {'status':'audited','mode':mode,'cells':{},'source_code_sha256':source_code_hashes,
              'checks':{'commands':len(meta['commands']),'zero_exit_commands':sum(c['exit_code']==0 for c in meta['commands']),
                        'preserved_artifact_hashes':len(files)},
              'limits':['Performance interpretation requires the formal run exit result and documented host/load context.']}
    need(set(summary)=={f'{shape}-{seed}-{count}' for shape,seed,count in combos}, 'summary cell set differs')
    for shape,seed,count in combos:
        key=f'{shape}-{seed}-{count}'
        source=root/f'{key}-source.json'
        source_data=load(source)
        need(source_data['version']==1 and len(source_data['events'])==count, f'{key}: source event count')
        source_hash=digest(source)
        observations={layout:[] for layout in ('json64','plain64','zstd64','zstd256')}
        receipts=[]; receipt_grids=[]; sidecars=[]
        for trial in all_trials:
            prefix=f'{key}-{trial}'
            layout_paths={layout:root/f'{prefix}-{layout}' for layout in observations}
            answer_hash=None
            for layout,path in layout_paths.items():
                header=load(path/'result.json')
                if answer_hash is None:answer_hash=header['answer_sha256']
                row,grid=audit_layout(path,source_hash,answer_hash,count,trial,mode)
                if trial>=0:observations[layout].append(row)
            if count==2048:
                receipt,grid=audit_receipt(root/f'{prefix}-receipt.json',source_hash,answer_hash,trial,mode)
                side=audit_sidecar(root,key,trial,source_hash,answer_hash,mode)
                if trial>=0:receipts.append(receipt);receipt_grids.append(grid);sidecars.append(side)
        saved=summary[key]
        baseline=observations['json64']
        json_bytes=statistics.median(r['layout_bytes']+r['metadata_bytes'] for r in baseline)
        json_query=statistics.median(r['p50_projected_ns'] for r in baseline)
        cell={'source_sha256':source_hash,'events':count,'measured_trials':len(trials),'layouts':{}}
        for layout,rows in observations.items():
            total=statistics.median(r['layout_bytes']+r['metadata_bytes'] for r in rows)
            with_index=statistics.median(r['layout_bytes']+r['metadata_bytes']+r['index_bytes'] for r in rows)
            projected=statistics.median(r['p50_projected_ns'] for r in rows)
            saving=None; build=None; publication=None; combined=None; break_even=None
            if layout!='json64':
                savings=[]
                for row in rows:
                    p=[s['timing']['wall_ns'] for s in row['samples'] if s['mode']=='projected' and s['family']==3]
                    i=[s['timing']['wall_ns'] for s in row['samples'] if s['mode']=='postings' and s['family']==3]
                    savings.append(statistics.median(p)-statistics.median(i))
                saving=statistics.median(savings)
                build=statistics.median(r['index_build']['wall_ns'] for r in rows)
                publication=statistics.median(r['index_publication']['wall_ns'] for r in rows)
                combined=statistics.median(r['index_build']['wall_ns']+r['index_publication']['wall_ns'] for r in rows)
                if saving>0:break_even=math.ceil(combined/saving)
            expected={'median_bytes':total,'median_bytes_with_index':with_index,'median_projected_query_ns':projected,
                      'byte_ratio_to_json':total/json_bytes,'projected_ratio_to_json':projected/json_query,
                      'registered_layout_gate':None if layout=='json64' else total<=0.8*json_bytes and projected<=1.1*json_query,
                      'rare_postings_median_saving_ns':saving,'postings_build_median_ns':build,
                      'postings_publication_median_ns':publication,'postings_build_plus_publication_median_ns':combined,
                      'measured_break_even_queries':break_even}
            for field,value in expected.items():equal(saved['layouts'][layout][field],value,f'{key} {layout} {field}')
            cell['layouts'][layout]={**expected,'projected_families':family_stats(rows,'projected')}
            if layout!='json64':cell['layouts'][layout]['postings_families']=family_stats(rows,'postings')
        if count==2048:
            paired=[]
            for row,grid in zip(receipts,receipt_grids):
                for i in range(128):
                    scalar=grid[('scalar',i)]; receipt=grid[('receipt',i)]
                    paired.append({'trial':row['trial'],'query':i,'family':i%8,'scalar_wall_ns':scalar['timing']['wall_ns'],
                                   'receipt_wall_ns':receipt['timing']['wall_ns'],'ratio':receipt['timing']['wall_ns']/scalar['timing']['wall_ns'],
                                   'receipt_page_bytes':receipt['page_bytes']})
            need(len(saved['receipt']['paired'])==len(paired),f'{key}: receipt pair count')
            for actual,want in zip(saved['receipt']['paired'],paired):
                for field,value in want.items():equal(actual[field],value,f'{key} receipt pair {field}')
            ratio_by_family={str(f):statistics.median(p['ratio'] for p in paired if p['family']==f) for f in range(8)}
            p99_by_family={str(f):rank([p['ratio'] for p in paired if p['family']==f],99) for f in range(8)}
            for f in range(8):
                equal(saved['receipt']['median_ratio_by_family'][str(f)],ratio_by_family[str(f)],f'{key} receipt family {f} median')
                equal(saved['receipt']['p99_ratio_by_family'][str(f)],p99_by_family[str(f)],f'{key} receipt family {f} p99')
            pairs=[]
            for trial,rows in zip(trials,sidecars):
                source=rows['source-cost'];baseline=rows['sidecar-baseline'];valid=rows['sidecar-valid']
                base_cpu=baseline['phase']['cpu_ns'];source_cpu=source['phase']['cpu_ns'];server_cpu=valid['phase']['cpu_ns']
                pairs.append({'trial':trial,'baseline_server_cpu_ns':base_cpu,'sidecar_source_cpu_ns':source_cpu,
                              'sidecar_server_cpu_ns':server_cpu,'hint_bytes':source['hint_bytes'],'source_bytes':source['source_bytes'],
                              'server_cpu_ratio':server_cpu/base_cpu,'total_cpu_ratio':(source_cpu+server_cpu)/base_cpu})
            need(len(saved['sidecar']['paired'])==len(pairs),f'{key}: sidecar pair count')
            for actual,want in zip(saved['sidecar']['paired'],pairs):
                for field,value in want.items():equal(actual[field],value,f'{key} sidecar pair {field}')
            median_server=statistics.median(p['server_cpu_ratio'] for p in pairs)
            median_total=statistics.median(p['total_cpu_ratio'] for p in pairs)
            for field,value in [('median_server_cpu_ratio',median_server),('median_total_cpu_ratio',median_total),
                                ('registered_server_gate',median_server<=0.9),('registered_total_gate',median_total<=1.1)]:
                equal(saved['sidecar'][field],value,f'{key} sidecar {field}')
            cell['receipt']={'median_ratio_by_family':ratio_by_family,'p99_ratio_by_family':p99_by_family,
                             'scalar_families':family_stats(receipts,'scalar'),'receipt_families':family_stats(receipts,'receipt')}
            cell['sidecar']={'median_server_cpu_ratio':median_server,'median_total_cpu_ratio':median_total,
                             'registered_server_gate':median_server<=0.9,'registered_total_gate':median_total<=1.1}
        result['cells'][key]=cell
    return result


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('run',type=Path)
    parser.add_argument('report',type=Path)
    parser.add_argument('--smoke',action='store_true')
    parser.add_argument('--root-exit',type=int)
    args=parser.parse_args()
    if not args.smoke:
        need(args.root_exit == 0, 'formal audit requires confirmed root exit 0')
    report=audit(args.run,'smoke' if args.smoke else 'formal')
    args.report.parent.mkdir(parents=True,exist_ok=True)
    args.report.write_text(json.dumps(report,indent=2)+'\n')
    print(f'audited {len(report["cells"])} cells; report {args.report}')


if __name__=='__main__':main()
