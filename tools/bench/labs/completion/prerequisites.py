"""Finite SDK/cgroup environment preflights; no host package installation."""
import argparse, inspect, json, os, subprocess, sys
from pathlib import Path
ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT/'tools'))
from resource_group import require_limits

def main():
    require_limits()
    ap=argparse.ArgumentParser(); ap.add_argument('kind',choices=['sdk','cgroup','container']); a=ap.parse_args()
    out=ROOT/'docs/experiments/benchmarks/data/lab-completion-run-01/coordinator'
    if a.kind=='sdk':
        venv=Path('/run/media/kmosoti/data/FabricO11y/sdk-completion-1.38.0')
        if venv.exists(): raise RuntimeError('fresh venv required')
        subprocess.run([sys.executable,'-m','venv',str(venv)],check=True)
        py=venv/'bin/python'
        subprocess.run([str(py),'-m','pip','install','--no-cache-dir','opentelemetry-sdk==1.38.0',
                        'opentelemetry-exporter-otlp-proto-http==1.38.0'],check=True,timeout=240)
        result=subprocess.check_output([str(py),'-m','pip','freeze'],text=True)
        (out/'sdk-freeze.txt').write_text(result)
        code='import inspect; from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter; print(inspect.getsource(OTLPSpanExporter))'
        source=subprocess.check_output([str(py),'-c',code],text=True)
        (out/'sdk-exporter-source.txt').write_text(source)
        print(result)
    elif a.kind=='cgroup':
        import cgroups
        parent=cgroups.delegate()
        group,limits=cgroups.subgroup(parent,'control',64*2**20,48*2**20,32,1)
        result=subprocess.run([sys.executable,'-c','import sys; sys.path.insert(0,sys.argv[1]); import cgroups; cgroups.enter(sys.argv[2]); print(cgroups.current())',str(Path(__file__).parent),str(group)],capture_output=True,text=True,check=True)
        if result.stdout.strip()!=str(group): raise RuntimeError('incorrect descendant placement')
        (out/'nested-cgroup-preflight.json').write_text(json.dumps({'limits':limits,'placement':result.stdout.strip(),'snapshot':cgroups.snapshot(parent)},indent=2))
        print(result.stdout.strip())
    else:
        import cgroups
        parent=cgroups.delegate()
        work=Path(os.environ['FABRIC_SCRATCH_ROOT'])/'podman';work.mkdir()
        command=['podman','--root',str(work/'root'),'--runroot',str(work/'runroot'),
                 '--storage-driver','vfs','--cgroup-manager','cgroupfs','--events-backend','file',
                 'run','--rm','--cgroup-parent','/'+str(parent.relative_to(cgroups.ROOT)),
                 '--cgroups','enabled','--memory','512m','--memory-swap','512m','--pids-limit','128',
                 '--cpus','2','docker.io/library/debian:trixie','cat','/etc/os-release']
        r=subprocess.run(command,capture_output=True,text=True,timeout=240)
        receipt={'command':command,'exit':r.returncode,'stdout':r.stdout,'stderr':r.stderr,
                 'cgroups':cgroups.snapshot(parent),'scope':'rootless disposable runtime preflight only'}
        (out/'container-preflight.json').write_text(json.dumps(receipt,indent=2))
        print(json.dumps(receipt))
        raise SystemExit(r.returncode)
if __name__=='__main__': main()
