"""Cheap executable controls and source binding before pressure workloads."""
import ast
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import job
import service
import remote_reduce

service.resource_group.require_limits()
out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=False)
if len(sys.argv)>2:
    service.BUILD = Path(sys.argv[2]).resolve(strict=True)
for path in Path(__file__).parent.glob('*.py'):
    ast.parse(path.read_text(), filename=str(path))
ledgers = {name: [] for name in (*job.base.HISTORY, 'native-frontier-01', 'previous-native-frontier-01')}
ledgers['lab-completion-run-01'] = [dict(id='catalog-fixture', state='passed', elapsed_s=100)]
ledgers['previous-native-frontier-01'] = [dict(state='passed', elapsed_s=50)]
ledgers['native-frontier-01'] = [dict(state='passed', elapsed_s=20)]
values = job.totals(ledgers)
assert values['remaining_campaign_s'] == 86400-170
assert values['remaining_frontier_s'] == 36000-170
assert values['remaining_round_s'] == 7200-20
rejected = []
for bad in (dict(state='running', elapsed_s=0), dict(state='passed'),
            dict(state='passed', elapsed_s=-1), dict(state='passed', elapsed_s=float('nan'))):
    try:
        job.totals(dict(ledgers, **{'previous-native-frontier-01': [bad]}))
    except RuntimeError:
        rejected.append(bad['state'] + ':' + str(bad.get('elapsed_s')))
    else:
        raise RuntimeError('invalid historical charge admitted')
for key in ('remaining_campaign_s','remaining_frontier_s','remaining_round_s'):
    try:
        job.base.admit(10, 'memory', 1, dict(values, **{key: 9}), dict.fromkeys(job.base.CAPS,0), 0, job.base.FREE_BYTES)
    except RuntimeError:
        rejected.append(key)
    else:
        raise RuntimeError('exhausted allowance admitted')
bins = Path(os.environ['CARGO_TARGET_DIR'])/'release'
hashes, source = service.frozen_build(bins)
command = [sys.executable,'-B',str(Path(service.__file__)),'--controls']
completed = subprocess.run(command, check=True, capture_output=True, text=True)
archive_controls = []
time_controls = []
remote_reduce.validate_times({(0,1):10},{(0,1):9},[((0,1),11,12,'ack')])
for defect, created, start, end in [('creation_before_event',8,11,12),
                                    ('send_before_creation',10,9,12),
                                    ('negative_rtt',10,12,11)]:
    try:
        remote_reduce.validate_times({(0,1):created},{(0,1):9},[((0,1),start,end,'ack')])
    except ValueError:
        time_controls.append(defect)
    else:
        raise RuntimeError('invalid event ordering admitted: '+defect)
original_scratch = os.environ['FABRIC_SCRATCH_ROOT']
with tempfile.TemporaryDirectory(prefix='archive-controls-', dir=original_scratch) as owned:
    os.environ['FABRIC_SCRATCH_ROOT'] = owned
    try:
        for defect in ('none','digest','missing','duplicate','traversal'):
            fixture = Path(owned)/defect
            fixture.mkdir()
            members = ['resources.json','sim/events.jsonl','sim/transcript.jsonl',
                       'sim/sim-summary.json','worker-result.json']
            if defect == 'missing': members.pop()
            if defect == 'duplicate': members.append(members[0])
            if defect == 'traversal': members.append('../escape')
            archive = fixture/'remote.tar.gz'
            with tarfile.open(archive,'w:gz') as tar:
                for name in members:
                    info = tarfile.TarInfo(name); info.size = 2
                    tar.addfile(info,io.BytesIO(b'{}'))
            sha = '0'*64 if defect == 'digest' else service.digest(archive)
            (fixture/'remote-archive.json').write_text(json.dumps(dict(bytes=archive.stat().st_size,sha256=sha)))
            try:
                paths, extracted, _ = remote_reduce.remote_inputs(fixture)
            except ValueError:
                if defect == 'none': raise
                archive_controls.append(defect)
            else:
                if defect != 'none': raise RuntimeError('archive defect accepted: '+defect)
                if len(paths) != 5: raise RuntimeError('archive baseline omitted required inputs')
                shutil.rmtree(extracted)
    finally:
        os.environ['FABRIC_SCRATCH_ROOT'] = original_scratch
(out/'summary.json').write_text(json.dumps(dict(rejected=rejected, cumulative_accounting=True,
    archive_controls_rejected=archive_controls,
    event_time_controls_rejected=time_controls,
    binaries=hashes, source=source, controls_command=command, controls_exit=completed.returncode,
    controls_stdout=completed.stdout), indent=2)+'\n')
print('source identities, pressure controls and seven admission controls passed')
