"""Exercise native candidate refusal paths without editing source."""
import pathlib,subprocess,tempfile
root=pathlib.Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix='fabric-candidate-') as d:
 p=pathlib.Path(d)/'candidate.txt'
 def invoke(*args):return subprocess.run(['cargo','run','--quiet','--locked','-p','xtask','--','candidate',*args],cwd=root,capture_output=True,text=True)
 r=invoke('capture',str(p));assert r.returncode==0,r.stderr
 original=p.read_text();p.write_text(original+'wrong toolchain\n')
 r=invoke('verify',str(p));assert r.returncode!=0 and 'stale candidate or toolchain' in r.stderr
 p.write_text(original);identity=original.splitlines()[0]
 work=p.parent/('verify-'+identity);work.mkdir()
 r=invoke('verify',str(p));assert r.returncode!=0 and 'already exists' in r.stderr
 assert work.is_dir(), 'Refusal must preserve pre-existing evidence'
 r=invoke('capture',str(root/'forbidden-candidate.txt'));assert r.returncode!=0
 assert not (root/'forbidden-candidate.txt').exists()
print('Candidate stale-input, owned-directory and source-tree refusal controls completed')
