"""Separate immutable-input regrade of the late-ACK observation counterexample."""
import json
import os
from pathlib import Path
import shutil
import sys
import remote
import service

service.resource_group.require_limits()
source=Path(sys.argv[1]).resolve(strict=True)
out=Path(sys.argv[2]);out.mkdir(parents=True,exist_ok=False)
work=Path(os.environ['FABRIC_SCRATCH_ROOT'])/'regrade';work.mkdir()
latest={'node00':dict(batch=75,acked_through=74,log_backlog_bytes=0)}
assert remote.drained(latest,{('node00',75):'hash'},1)
assert not remote.drained(latest,{('node00',74):'hash'},1)
assert not remote.drained({'node00':dict(batch=75,acked_through=74,log_backlog_bytes=1)},{('node00',75):'hash'},1)
assert not remote.drained({}, {('node00',75):'hash'},1)
controls=dict(origin='edge-real-02 final native cycle followed by later ACK',
    trace=[dict(type='cycle',batch=75,acked_through=74,log_backlog_bytes=0),dict(type='delivery',sequence=75,status='ack')],
    positive=True,rejected=['missing_final_ACK','source_backlog','missing_node'])
(out/'controls.json').write_text(json.dumps(controls,indent=2)+'\n')
files=[source/'recovered.jsonl',source/'quiescent.json',source/'remote/sources.jsonl.gz',source/'remote/worker-result.json',
       *sorted((source/'remote').glob('node*-stdout.jsonl.gz'))]
hashes={str(p.relative_to(source)):service.digest(p) for p in files}
for p in files:
    target=work/p.relative_to(source);target.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(p,target)
observer=service.observation.Observer();observer.finals=json.loads((work/'quiescent.json').read_text())
remote.grade(work,'real',observer)
assert hashes=={str(p.relative_to(source)):service.digest(p) for p in files}
for p in work.iterdir():
    if p.is_file() and p.name!='recovered.jsonl':
        shutil.copyfile(p,out/p.name);assert service.digest(p)==service.digest(out/p.name)
(out/'input-sha256.json').write_text(json.dumps(dict(source=str(source),inputs=hashes),indent=2)+'\n')
(out/'harness-sha256.json').write_text(json.dumps({str(p):service.digest(p) for p in [Path(__file__),Path(remote.__file__),Path(service.__file__)]},indent=2)+'\n')
shutil.rmtree(work)
(out/'cleanup.json').write_text(json.dumps(dict(removed=not work.exists(),original_unchanged=True))+'\n')
print('separate late-ACK regrade and three negative controls passed')
