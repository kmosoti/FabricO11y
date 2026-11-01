import tempfile,pathlib,subprocess,shutil
with tempfile.TemporaryDirectory(prefix='fabric-jj-') as d:
 p=pathlib.Path(d)
 def jj(*a):return subprocess.check_output(['jj','--config','user.name="Fixture"','--config','user.email="fixture@example.invalid"',*a],cwd=p,text=True,stderr=subprocess.DEVNULL).strip()
 jj('git','init','--colocate')
 with (p/'.git/info/exclude').open('a') as f:f.write('\n/backlog/*\n')
 (p/'backlog').mkdir();(p/'backlog/private').write_text('local')
 (p/'old').write_text('archive')
 jj('util','snapshot');old=jj('log','--no-graph','-r','@','-T','commit_id')
 jj('bookmark','create','archive/test','-r',old);jj('new',old)
 (p/'old').unlink();(p/'fresh').write_text('new')
 jj('util','snapshot');new=jj('log','--no-graph','-r','@','-T','commit_id')
 assert jj('file','show','-r','archive/test','old')=='archive'
 assert jj('file','list')=='fresh' and old!=new
 # Negative control: a changed source must invalidate the captured identity.
 (p/'fresh').write_text('changed');jj('util','snapshot')
 assert jj('log','--no-graph','-r','@','-T','commit_id')!=new
 assert (p/'backlog/private').read_text()=='local'
print('Jujutsu archive, exclusion and rewrite controls completed')
