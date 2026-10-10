"""Bounded exact failure archives and explicit historical-map reconciliation."""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import tarfile
import time


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def inventory(root):
    result = {}
    for path in sorted(root.rglob('*')):
        name = str(path.relative_to(root))
        if path.is_symlink():
            raise RuntimeError('linked evidence member')
        if path.is_dir():
            result[name] = dict(type='directory')
        elif path.is_file():
            result[name] = dict(type='file', bytes=path.stat().st_size, sha256=sha(path))
        else:
            raise RuntimeError('special evidence member')
    return result


def verify(archive, expected, deadline):
    seen = set()
    with tarfile.open(archive, 'r:gz') as tar:
        for member in tar:
            if time.monotonic() >= deadline:
                raise TimeoutError('archive verification deadline')
            if member.name in seen or member.name not in expected:
                raise RuntimeError('duplicate or unlisted archive member')
            seen.add(member.name)
            original = expected[member.name]
            if original['type'] == 'directory':
                if not member.isdir():
                    raise RuntimeError('directory changed type')
            else:
                if not member.isfile() or member.size != original['bytes']:
                    raise RuntimeError('file changed type/size')
                with tar.extractfile(member) as stream:
                    if hashlib.file_digest(stream, 'sha256').hexdigest() != original['sha256']:
                        raise RuntimeError('archive changed bytes')
    if seen != set(expected):
        raise RuntimeError('missing archive member')


def preserve(work, out, cap, deadline):
    archive, manifest = out/'failure-tree.tar.gz', out/'failure-tree-members.json'
    if archive.exists() or manifest.exists():
        raise RuntimeError('fresh archive destination required')
    members = inventory(work)
    encoded = (json.dumps(members, indent=2)+'\n').encode()
    class BudgetWriter:
        def __init__(self, stream):
            self.stream = stream
        def write(self, payload):
            if time.monotonic() >= deadline-10 or self.stream.tell()+len(payload)+len(encoded) > cap:
                raise RuntimeError('archive plus member metadata exceeds allowance/deadline')
            return self.stream.write(payload)
        def __getattr__(self, name):
            return getattr(self.stream, name)
    try:
        if sum(m.get('bytes',0) for m in members.values()) > 512*2**20:
            raise RuntimeError('raw failed state exceeds inherited bound')
        with archive.open('xb') as raw:
            with gzip.GzipFile(fileobj=BudgetWriter(raw), mode='wb', mtime=0, compresslevel=1) as zipped:
                with tarfile.open(fileobj=zipped, mode='w|') as tar:
                    for name, item in members.items():
                        info = tar.gettarinfo(str(work/name), arcname=name)
                        info.mtime = 0
                        if item['type']=='directory':
                            tar.addfile(info)
                        else:
                            with (work/name).open('rb') as source:
                                tar.addfile(info, source)
            raw.flush()
            os.fsync(raw.fileno())
        manifest.write_bytes(encoded)
        verify(archive, members, deadline-5)
        if inventory(work) != members:
            raise RuntimeError('original tree changed before removal')
        if archive.stat().st_size+manifest.stat().st_size > cap:
            raise RuntimeError('final archive allowance')
        receipt = dict(archive_sha256=sha(archive), verified_members=len(members), removed=True)
        shutil.rmtree(work)
        return receipt
    except Exception as exc:
        archive.unlink(missing_ok=True)
        manifest.unlink(missing_ok=True)
        return dict(removed=False, error=repr(exc), original=str(work))


def controls(work):
    source = work/'original'
    source.mkdir()
    (source/'empty').mkdir()
    (source/'file').write_bytes(b'original')
    expected = inventory(source)
    rejected = []
    for variant in ('valid', 'missing', 'changed', 'added', 'duplicate', 'special'):
        path = work/(variant+'.tar.gz')
        with tarfile.open(path,'w:gz') as tar:
            directory = tarfile.TarInfo('empty')
            directory.type = tarfile.DIRTYPE
            tar.addfile(directory)
            names = ([] if variant=='missing' else ['file']) + (['file'] if variant=='duplicate' else []) + (['added'] if variant=='added' else [])
            for name in names:
                item = tarfile.TarInfo(name)
                payload = b'modified' if variant=='changed' else b'original'
                if variant=='special':
                    item.type, item.linkname = tarfile.SYMTYPE, 'elsewhere'
                    tar.addfile(item)
                else:
                    item.size = len(payload)
                    tar.addfile(item,io.BytesIO(payload))
        try:
            verify(path,expected,time.monotonic()+10)
        except RuntimeError:
            if variant=='valid':
                raise
            rejected.append(variant)
        else:
            if variant!='valid':
                raise RuntimeError('archive defect accepted: '+variant)
    fail_out = work/'cap-failure'
    fail_out.mkdir()
    failure = preserve(source,fail_out,1,time.monotonic()+20)
    if failure['removed'] or inventory(source)!=expected or list(fail_out.iterdir()):
        raise RuntimeError('budget rejection lost originals or left partial archive')
    success_out = work/'valid-preservation'
    success_out.mkdir()
    success = preserve(source,success_out,1024*1024,time.monotonic()+20)
    if not success['removed'] or source.exists():
        raise RuntimeError('valid exact preservation failed')
    return dict(rejected=rejected+['archive_metadata_budget'], exact_preservation=True,
                empty_directories_preserved=True, originals_retained_on_budget_failure=True)


def main():
    import sys
    root = Path(__file__).resolve().parents[4]
    sys.path.insert(0,str(root/'tools'))
    from resource_group import require_limits
    require_limits()
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--out',type=Path,required=True)
    args = ap.parse_args()
    args.out.mkdir(parents=True,exist_ok=False)
    work = Path(os.environ['FABRIC_SCRATCH_ROOT'])/'archive-controls'
    work.mkdir()
    checks = controls(work)
    shutil.rmtree(work)
    records = {}
    for plan in ('scan','walk'):
        directory = root/'docs/experiments/benchmarks/data/native-frontier-01/memory'/f'readiness-{plan}-01'
        prior = json.loads((directory/'compact-sha256.json').read_text())
        current = {p.name:sha(p) for p in directory.iterdir() if p.is_file()}
        mismatches = {name:dict(recorded=value,current=current.get(name)) for name,value in prior.items() if current.get(name)!=value}
        if set(mismatches)-{'cgroup-final.json'}:
            raise RuntimeError('unexpected historical artifact changed')
        records[plan] = dict(original_map_sha256=current['compact-sha256.json'],
            original_mismatches=mismatches, current_artifact_sha256=current,
            earlier_overwritten_resource_snapshot_recovered=False,
            original_files_modified=False)
    (args.out/'reconciliation.json').write_text(json.dumps(dict(controls=checks,datasets=records),indent=2)+'\n')
    shutil.copyfile(Path(__file__),args.out/'evidence.py')
    print(json.dumps(dict(controls=checks,mismatches={k:list(v['original_mismatches']) for k,v in records.items()})))


if __name__=='__main__':
    main()
