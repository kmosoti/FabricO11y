#!/usr/bin/env python3
"""Non-vacuity probes against one preserved smoke result; no benchmark rerun."""
import copy
import json
from pathlib import Path
import shutil
import sys
import tempfile
from run_research_costs import checked, run, summarize, validate_layout_artifacts, validate_pair


def reject(label, operation):
    try:
        operation()
    except RuntimeError as exc:
        print(f'{label}: rejected ({exc})')
    else:
        raise AssertionError(f'{label}: accepted corrupted evidence')


def main():
    if len(sys.argv) != 2:
        raise SystemExit('usage: python3 -B tools/bench/test_research_costs.py SMOKE_OUTPUT')
    root = Path(sys.argv[1]).resolve()
    source = root / 'mixed-201-2048-source.json'
    receipt = root / 'mixed-201-2048-0-receipt.json'
    layout = root / 'mixed-201-2048-0-json64' / 'result.json'
    zstd = root / 'mixed-201-2048-0-zstd64' / 'result.json'
    source_hash = __import__('hashlib').sha256(source.read_bytes()).hexdigest()
    good = [json.loads(receipt.read_text()), json.loads(layout.read_text())]
    validate_pair(source_hash, good)
    good_zstd = json.loads(zstd.read_text())
    checked(zstd, 'layout', 'smoke', 'mixed', 201, 0, 384)
    corrupted = copy.deepcopy(good)
    corrupted[1]['source_sha256'] = '0' * 64
    reject('source hash mutation', lambda: validate_pair(source_hash, corrupted))
    corrupted = copy.deepcopy(good)
    corrupted[1]['answer_sha256'] = '0' * 64
    reject('paired answer hash mutation', lambda: validate_pair(source_hash, corrupted))
    with tempfile.TemporaryDirectory(prefix='fabric-cost-validator-') as temp:
        temp = Path(temp)
        bad = temp / 'bad.json'
        data = copy.deepcopy(good[0]); data['checks_ok'] = False
        bad.write_text(json.dumps(data))
        reject('corrupted result', lambda: checked(bad, 'receipt', 'smoke', 'mixed', 201, 0, 256))
        reject('failed child', lambda: run(['/bin/false'], temp, []))
        mutations = {}
        bad_p50 = copy.deepcopy(good_zstd)
        bad_p50['p50_projected_ns'] = 100_000_000
        mutations['changed p50 flips layout gate'] = bad_p50
        duplicate = copy.deepcopy(good_zstd)
        duplicate['samples'][127] = copy.deepcopy(duplicate['samples'][0])
        mutations['duplicate/missing sample'] = duplicate
        negative = copy.deepcopy(good_zstd)
        negative['samples'][0]['timing']['wall_ns'] = -999
        mutations['negative timing'] = negative
        wrong_count = copy.deepcopy(good_zstd)
        wrong_count['samples'][3]['matches'] += 1
        mutations['wrong matched count'] = wrong_count
        bool_time = copy.deepcopy(good_zstd)
        bool_time['samples'][0]['timing']['wall_ns'] = True
        mutations['boolean timing'] = bool_time
        for label, item in mutations.items():
            path=temp/(label.replace(' ','_').replace('/','_')+'.json')
            path.write_text(json.dumps(item))
            reject(label,lambda path=path: checked(path,'layout','smoke','mixed',201,0,384))
    with tempfile.TemporaryDirectory(prefix='fabric-cost-summary-probe-',dir=root.parent) as temp:
        temporary=Path(temp)
        shutil.copytree(root,temporary,dirs_exist_ok=True)
        layout_dir=temporary/'mixed-201-2048-0-zstd64'
        anchor=layout_dir/'table-anchor.json'
        original_anchor=anchor.read_bytes()
        anchor.write_bytes(original_anchor+b' ')
        reject('persisted anchor byte mutation',lambda: validate_layout_artifacts(layout_dir,good_zstd))
        anchor.write_bytes(original_anchor)
        altered=temporary/zstd.relative_to(root)
        altered.write_text(json.dumps(bad_p50))
        reject('summary rejects changed p50',lambda: summarize(temporary,[('mixed',201,2048)],[0]))
    print('research cost validator probes passed')


if __name__ == '__main__':
    main()
