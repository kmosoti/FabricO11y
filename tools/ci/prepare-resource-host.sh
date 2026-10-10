#!/bin/bash
# Hosted Linux runner only: make disk-backed mounted storage and user cgroups.
set -euo pipefail
storage=/run/media/kmosoti/data
[ "$(stat -fc %T /sys/fs/cgroup)" = cgroup2fs ]
case "$(findmnt -no FSTYPE -T /mnt)" in tmpfs|ramfs) echo '/mnt must be disk backed' >&2; exit 1;; esac
sudo mkdir -p "$storage"
if ! mountpoint -q "$storage"; then sudo mount --bind /mnt "$storage"; fi
sudo mkdir -p "$storage/FabricO11y"
sudo chown "$(id -u):$(id -g)" "$storage/FabricO11y"
mkdir -p "$storage/FabricO11y/toolchain-cache/cargo" "$storage/FabricO11y/toolchain-cache/rustup" "$storage/FabricO11y/results" "$storage/FabricO11y/tools/ui"
# The launcher still uses the local 20 GiB ceiling. On a smaller hosted
# runner, a stricter verified ancestor contains its whole descendant tree.
mem_total_kib=$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)
parent_cap=$((mem_total_kib * 1024 * 3 / 4))
[ "$parent_cap" -le $((12 * 1024 * 1024 * 1024)) ] || parent_cap=$((12 * 1024 * 1024 * 1024))
[ "$parent_cap" -ge $((1024 * 1024 * 1024)) ] || { echo 'insufficient runner RAM' >&2; exit 1; }
page_bytes=$(getconf PAGESIZE)
parent_cap=$((parent_cap / page_bytes * page_bytes))
parent_high=$((parent_cap * 5 / 6 / page_bytes * page_bytes))
parent_cpu_percent=$(($(nproc) * 100))
parent_unit="user-$(id -u).slice"
sudo systemctl set-property "$parent_unit" MemoryAccounting=yes MemoryHigh="$parent_high" MemoryMax="$parent_cap" MemorySwapMax=0 CPUAccounting=yes CPUQuota="${parent_cpu_percent}%"
sudo loginctl enable-linger "$(id -un)"
sudo systemctl start "user@$(id -u).service"
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
python3 - "$parent_group" "$parent_cap" "$parent_high" "$parent_cpu_percent" <<'PYRECEIPT'
import json, sys, shutil
from pathlib import Path
path = Path(sys.argv[1])
receipt = {'role':'GitHub CI verified host-sized ancestor; local child ceiling unchanged',
           'parent_cgroup':str(path),'configured_memory_max':int(sys.argv[2]),
           'configured_memory_high':int(sys.argv[3]),'cpu_quota_percent':int(sys.argv[4]),
           'meminfo':Path('/proc/meminfo').read_text(),
           'actual':{name:(path/name).read_text().strip() for name in ('memory.max','memory.high','memory.swap.max','cpu.max')},
           'disk':dict(zip(('total','used','free'),shutil.disk_usage('/run/media/kmosoti/data/FabricO11y'))),
           'build_environment':{'CARGO_BUILD_JOBS':'2','RUST_TEST_THREADS':'2','CARGO_INCREMENTAL':'0','CARGO_PROFILE_DEV_DEBUG':'0','CARGO_PROFILE_TEST_DEBUG':'0'},
           'exit':0}
Path('target/resource-containment/runs/ci-parent.json').write_text(json.dumps(receipt,indent=2)+'\n')
PYRECEIPT
{
  echo "XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR"
  echo "DBUS_SESSION_BUS_ADDRESS=$DBUS_SESSION_BUS_ADDRESS"
  echo "CARGO_HOME=$storage/FabricO11y/toolchain-cache/cargo"
  echo "RUSTUP_HOME=$storage/FabricO11y/toolchain-cache/rustup"
  echo "CARGO_BUILD_JOBS=2"
  echo "RUST_TEST_THREADS=2"
  echo "CARGO_INCREMENTAL=0"
  echo "CARGO_PROFILE_DEV_DEBUG=0"
  echo "CARGO_PROFILE_TEST_DEBUG=0"
} >> "${GITHUB_ENV:?GitHub runner environment required}"
echo "$storage/FabricO11y/toolchain-cache/cargo/bin" >> "$GITHUB_PATH"
