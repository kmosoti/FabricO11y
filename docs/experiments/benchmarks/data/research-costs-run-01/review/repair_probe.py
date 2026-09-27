import copy
import importlib.util
import json
import pathlib
import shutil
import subprocess
import tempfile

ROOT = pathlib.Path('/home/kmosoti/fabric-cost-repair')
SMOKE = pathlib.Path('/home/kmosoti/fabric-cost-repair-smoke-20260927-c')
HERE = pathlib.Path(__file__).parent
spec = importlib.util.spec_from_file_location('cost_runner', ROOT / 'tools/bench/run_research_costs.py')
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)
result = []

def probe(label, mutate, expected):
    source = SMOKE / 'mixed-201-2048-0-zstd64/result.json'
    data = json.loads(source.read_text())
    mutate(data)
    with tempfile.TemporaryDirectory(dir=HERE) as temp:
        path = pathlib.Path(temp) / 'result.json'
        path.write_text(json.dumps(data))
        try:
            runner.checked(path, 'layout', 'smoke', 'mixed', 201, 0, 384)
        except Exception as error:
            outcome = 'rejected'
            message = str(error)
        else:
            outcome = 'accepted'
            message = ''
    result.append({'probe': label, 'input': expected, 'outcome': outcome, 'message': message})

probe('derived p50', lambda d: d.__setitem__('p50_projected_ns', 100000000), 'p50_projected_ns=100000000')
probe('duplicate sample', lambda d: d['samples'].__setitem__(127, copy.deepcopy(d['samples'][0])), 'samples[127]=samples[0]')
probe('negative wall', lambda d: d['samples'][0]['timing'].__setitem__('wall_ns', -999), 'samples[0].timing.wall_ns=-999')
probe('wrong match count', lambda d: d['samples'][0].__setitem__('matches', d['samples'][0]['matches'] + 1), 'samples[0].matches+=1')
probe('bad family', lambda d: d['samples'][0].__setitem__('family', 7), 'samples[0].family=7')
probe('missing mode', lambda d: d['samples'][0].__setitem__('mode', 'projected'), 'samples[0].mode=projected')

sample_dir = SMOKE / 'mixed-201-2048-0-zstd64'
sample_row = json.loads((sample_dir / 'result.json').read_text())
with tempfile.TemporaryDirectory(dir=HERE) as temp:
    directory = pathlib.Path(temp)
    for name in ('table.parquet', 'table-anchor.json', 'postings.json', 'postings-digest.txt'):
        shutil.copy2(sample_dir / name, directory / name)
    runner.validate_layout_artifacts(directory, sample_row)
    result.append({'probe': 'untouched persisted artifacts', 'outcome': 'accepted'})
    for name in ('table-anchor.json', 'postings-digest.txt'):
        path = directory / name
        original = path.read_bytes()
        altered = bytearray(original)
        altered[-1] = 48 if altered[-1] != 48 else 49
        path.write_bytes(altered)
        try:
            runner.validate_layout_artifacts(directory, sample_row)
        except Exception as error:
            result.append({'probe': 'same-length mutation ' + name, 'outcome': 'rejected', 'message': str(error)})
        else:
            result.append({'probe': 'same-length mutation ' + name, 'outcome': 'accepted'})
        path.write_bytes(original)

for n, stage in ((4, 'anchor file sync'), (7, 'index digest file sync'), (8, 'index parent sync')):
    output = HERE / f'repair-fsync-{n}'
    command = [str(ROOT / 'tools/layout-probe/target/release/layout-cost'), 'smoke', 'mixed', '201', '2048', str(200+n), 'zstd64', str(SMOKE / 'mixed-201-2048-source.json'), str(output)]
    env = {'LD_PRELOAD': str(HERE / 'fail_fsync.so'), 'FAIL_FSYNC_AT': str(n)}
    import os
    run = subprocess.run(command, env={**os.environ, **env}, capture_output=True, text=True)
    result.append({'probe': stage, 'input': {'FAIL_FSYNC_AT': n, 'command': command}, 'exit_code': run.returncode, 'stderr': run.stderr.strip(), 'result_exists': (output / 'result.json').exists()})

(HERE / 'repair-probes.json').write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps(result, indent=2))
