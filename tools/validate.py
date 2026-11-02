import json, pathlib, subprocess
r=pathlib.Path(__file__).resolve().parents[1]
hooks=json.loads((r/'.codex/hooks.json').read_text())
assert set(hooks['hooks'])=={'SessionStart'}
p=subprocess.run(['python3',str(r/'.codex/hooks/context.py')],capture_output=True,text=True,check=True)
assert 'systemMessage' in json.loads(p.stdout)
assert not any((r/p).exists() for p in ['src','crates/fabric-server','vendor','packaging','tools/resource_group.py'])
assert (r/'.agents/skills/jj-workflow/SKILL.md').read_text().startswith('---\nname: jj-workflow\n')
print('Repository configuration and hook checks completed')

import tomllib
a=tomllib.loads((r/".codex/agents/research.toml").read_text())
assert a["name"] == "fabric_research"
manifest=json.loads(subprocess.check_output(["cargo","metadata","--no-deps","--format-version","1"],cwd=r,text=True))
assert all(p["license"]=="Apache-2.0" for p in manifest["packages"]), "Package license declaration missing or incompatible; this is not a transitive audit"
