"""Authenticate complete launcher-retained failure trees before cleanup."""
import gzip
import json
import os
from pathlib import Path
import shutil
import sys
import tarfile
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0,str(ROOT/'tools/bench/labs/readiness_service'))
import evidence
sys.path.insert(0,str(ROOT/'tools'))
import resource_group


def main():
    resource_group.require_limits()
    out = Path(sys.argv[1])
    out.mkdir(parents=True,exist_ok=False)
    results = []
    deadline = time.monotonic()+240
    for unit in sys.argv[2:]:
        receipt = ROOT/'target/resource-containment/runs'/(unit+'.json')
        meta = json.loads(receipt.read_text())
        source = Path(meta['retained_failure_evidence']).resolve(strict=True)
        if meta['exit']==0 or source.parent!=resource_group.STORAGE/'evidence' or source.name!=unit:
            raise RuntimeError('not an authenticated owned failed tree')
        dest = out/unit
        dest.mkdir()
        members = evidence.inventory(source)
        encoded = (json.dumps(members,indent=2)+'\n').encode()
        if sum(v.get('bytes',0) for v in members.values())>2*2**30:
            raise RuntimeError('failed tree exceeds scoped 2GiB preservation bound')
        archive = dest/'failure.tar.gz'
        class Writer:
            def __init__(self,stream): self.stream=stream
            def write(self,data):
                if self.stream.tell()+len(data)+len(encoded)>768*2**20 or time.monotonic()>deadline-10:
                    raise RuntimeError('archive metadata/byte/deadline limit')
                return self.stream.write(data)
            def __getattr__(self,name): return getattr(self.stream,name)
        with archive.open('xb') as raw:
            with gzip.GzipFile(fileobj=Writer(raw),mode='wb',mtime=0,compresslevel=1) as zipped:
                with tarfile.open(fileobj=zipped,mode='w|') as tar:
                    for name,value in members.items():
                        info = tar.gettarinfo(str(source/name),arcname=name)
                        if value.get('type')=='directory': tar.addfile(info)
                        else:
                            with (source/name).open('rb') as stream: tar.addfile(info,stream)
            raw.flush(); os.fsync(raw.fileno())
        (dest/'members.json').write_bytes(encoded)
        # Same exact-members/content verifier whose six controls were executed
        # in the preceding service evidence round; no verifier change here.
        typed = {k:dict(v,type=v.get('type','file')) for k,v in members.items()}
        evidence.verify(archive,typed,deadline-5)
        if evidence.inventory(source)!=members:
            raise RuntimeError('original failure changed while preserving')
        record = dict(unit=unit,source=str(source),archive=str(archive),
            archive_sha256=evidence.sha(archive),members=len(members),
            original_bytes=sum(v.get('bytes',0) for v in members.values()),
            archive_bytes=archive.stat().st_size,launcher_receipt_sha256=evidence.sha(receipt))
        (dest/'before-removal.json').write_text(json.dumps(record,indent=2)+'\n')
        shutil.rmtree(source)
        record['removed']=not source.exists()
        results.append(record)
        (out/'summary.json').write_text(json.dumps(results,indent=2)+'\n')
    print(json.dumps(results))


if __name__=='__main__': main()
