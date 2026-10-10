"""Serial completion selectors under the existing resource launcher."""
import json
import os
import subprocess
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from resource_group import require_limits
require_limits()
format_cmd = ['rustfmt','--edition','2024','src/spindle/overlap_tests.rs',
    'crates/fabric-server/examples/coupled_pruning_probe.rs',
    'crates/fabric-server/examples/coupled_c5_server.rs',
    'crates/fabric-server/examples/coupled_pending_seed.rs']
print(json.dumps({'command':format_cmd}), flush=True)
subprocess.run(format_cmd, check=True)
for setting in ('8','16','32'):
    command = ['cargo','test','--offline','--locked','-p','fabric-server','--lib','experimental_run_limit','--','--nocapture']
    env=dict(os.environ,FABRIC_RUN_MIB_EXPERIMENT=setting)
    print(json.dumps({'command':command,'FABRIC_RUN_MIB_EXPERIMENT':setting}), flush=True)
    result=subprocess.run(command,env=env)
    print(json.dumps({'setting':setting,'exit':result.returncode}),flush=True)
    if result.returncode:raise SystemExit(result.returncode)
