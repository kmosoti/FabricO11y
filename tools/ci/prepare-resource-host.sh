#!/bin/bash
# Hosted Linux runner only: make disk-backed storage and user cgroups.
set -euo pipefail
export FABRIC_STORAGE_ROOT=${FABRIC_STORAGE_ROOT:-/mnt/fabrico11y}
storage=$FABRIC_STORAGE_ROOT
[ "$(stat -fc %T /sys/fs/cgroup)" = cgroup2fs ]
sudo mkdir -p "$storage"
sudo chown "$(id -u):$(id -g)" "$storage"
mkdir -p "$storage/toolchain-cache/cargo" "$storage/toolchain-cache/rustup" "$storage/results" "$storage/tools/ui"
# The launcher sizes its own cap to the host. A stricter verified ancestor, sized
# by the same function, contains the whole descendant tree; it also refuses a RAM-backed root.
limits=$(PYTHONPATH=tools python3 -B -c 'import resource_group as r; r.require_storage(); print(*r.memory_limits(ceiling=12 << 30))')
read -r parent_high parent_cap <<< "$limits"
parent_cpu_percent=$(($(nproc) * 100))
parent_unit="user-$(id -u).slice"
sudo systemctl set-property "$parent_unit" MemoryAccounting=yes MemoryHigh="$parent_high" MemoryMax="$parent_cap" MemorySwapMax=0 CPUAccounting=yes IOAccounting=yes TasksAccounting=yes CPUQuota="${parent_cpu_percent}%"
sudo loginctl enable-linger "$(id -un)"
sudo systemctl start "user@$(id -u).service"
# Ubuntu's default user-manager delegation can omit io. The browser/soak
# descendants require all four controllers; configure the hosted manager and
# verify availability instead of dropping an unsupported controller silently.
sudo systemctl set-property --runtime "user@$(id -u).service" 'Delegate=cpu io memory pids'
user_relative=$(systemctl show -P ControlGroup "user@$(id -u).service")
user_group="/sys/fs/cgroup$user_relative"
python3 - "$user_group" <<'PYDELEGATE'
import sys
from pathlib import Path
path = Path(sys.argv[1])
available = set((path/'cgroup.controllers').read_text().split())
required = {'cpu', 'io', 'memory', 'pids'}
if not required <= available:
    raise SystemExit(f'hosted user-manager delegation missing {sorted(required-available)}; available={sorted(available)}')
print(f'verified hosted user-manager controllers: {sorted(available)}')
PYDELEGATE
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
systemctl --user show-environment >/dev/null
parent_relative=$(systemctl show -P ControlGroup "$parent_unit")
parent_group="/sys/fs/cgroup$parent_relative"
[ "$(cat "$parent_group/memory.max")" = "$parent_cap" ]
[ "$(cat "$parent_group/memory.high")" = "$parent_high" ]
[ "$(cat "$parent_group/memory.swap.max")" = 0 ]
read -r quota period < "$parent_group/cpu.max"
[ "$quota" != max ] && [ "$quota" -le $((period * parent_cpu_percent / 100)) ]
mkdir -p target/resource-containment/runs
python3 - "$parent_group" "$parent_cap" "$parent_high" "$parent_cpu_percent" "$user_group" <<'PYRECEIPT'
import json, os, sys, shutil
from pathlib import Path
path = Path(sys.argv[1])
receipt = {'role':'GitHub CI verified host-sized ancestor of the launcher groups',
           'user_manager_cgroup':sys.argv[5],
           'user_manager_controllers':(Path(sys.argv[5])/'cgroup.controllers').read_text().split(),
           'parent_cgroup':str(path),'configured_memory_max':int(sys.argv[2]),
           'configured_memory_high':int(sys.argv[3]),'cpu_quota_percent':int(sys.argv[4]),
           'meminfo':Path('/proc/meminfo').read_text(),
           'actual':{name:(path/name).read_text().strip() for name in ('memory.max','memory.high','memory.swap.max','cpu.max')},
           'disk':dict(zip(('total','used','free'),shutil.disk_usage(os.environ['FABRIC_STORAGE_ROOT']))),
           'build_environment':{'CARGO_BUILD_JOBS':'2','RUST_TEST_THREADS':'2','CARGO_INCREMENTAL':'0','CARGO_PROFILE_DEV_DEBUG':'0','CARGO_PROFILE_TEST_DEBUG':'0'},
           'exit':0}
Path('target/resource-containment/runs/ci-parent.json').write_text(json.dumps(receipt,indent=2)+'\n')
PYRECEIPT
{
  echo "XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR"
  echo "DBUS_SESSION_BUS_ADDRESS=$DBUS_SESSION_BUS_ADDRESS"
  echo "FABRIC_STORAGE_ROOT=$storage"
  echo "CARGO_HOME=$storage/toolchain-cache/cargo"
  echo "RUSTUP_HOME=$storage/toolchain-cache/rustup"
  echo "CARGO_BUILD_JOBS=2"
  echo "RUST_TEST_THREADS=2"
  echo "CARGO_INCREMENTAL=0"
  echo "CARGO_PROFILE_DEV_DEBUG=0"
  echo "CARGO_PROFILE_TEST_DEBUG=0"
} >> "${GITHUB_ENV:?GitHub runner environment required}"
echo "$storage/toolchain-cache/cargo/bin" >> "$GITHUB_PATH"
