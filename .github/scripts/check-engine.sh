#!/bin/sh
# Functional engine integration. Two-host resource qualification runs separately.
set -eu
engine_tools="${FABRIC_ENGINE_TOOLS:-$HOME/.local/share/fabrico11y/engine-tools}"
engine_binary="${FABRIC_ENGINE_BINARY:-$engine_tools/greptime-linux-amd64-v1.2.1/greptime}"
if [ ! -x "$engine_binary" ]; then
  mkdir -p "$engine_tools"
  curl --proto '=https' --tlsv1.2 --retry 3 -fLsS https://github.com/GreptimeTeam/greptimedb/releases/download/v1.2.1/greptime-linux-amd64-v1.2.1.tar.gz -o "$engine_tools/greptime-v1.2.1.tar.gz"
  printf '%s  %s\n' 062d5beb13de4991c4fec201b2e5071518bf4ca8f0d52947eba757a74c2a5420 "$engine_tools/greptime-v1.2.1.tar.gz" | sha256sum -c -
  tar -xzf "$engine_tools/greptime-v1.2.1.tar.gz" -C "$engine_tools"
fi
printf '%s  %s\n' a7ca4d5ee33bf7605dc69a7be46ae3ad34c5fba27d14edacf780af285dad179d "$engine_binary" | sha256sum -c -
"$engine_binary" --version | grep -Fx 'version: 1.2.1'
engine_trial=$(mktemp -d "${TMPDIR:-/var/tmp}/fabric-engine-XXXXXXXX")
engine_pid=''
cleanup() {
  engine_status=$?
  trap - EXIT INT TERM
  if [ -n "$engine_pid" ]; then
    kill "$engine_pid" 2>/dev/null || true
    wait "$engine_pid" 2>/dev/null || true
  fi
  if [ "$engine_status" -eq 0 ]; then
    rm -rf -- "$engine_trial"
  else
    printf 'Engine failure evidence retained at %s\n' "$engine_trial" >&2
  fi
  exit "$engine_status"
}
trap cleanup EXIT INT TERM
cat > "$engine_trial/config.toml" <<CONFIG
enable_telemetry = false
[http]
addr = "127.0.0.1:19116"
timeout = "5s"
body_limit = "1MiB"
enable_cors = false
[grpc]
bind_addr = "127.0.0.1:19117"
[mysql]
enable = false
[postgres]
enable = false
[storage]
data_home = "$engine_trial/data"
[wal]
provider = "raft_engine"
dir = "$engine_trial/data/wal"
sync_write = true
file_size = "16MB"
purge_threshold = "64MB"
[query]
parallelism = 1
memory_pool_size = "256MB"
[[region_engine]]
[region_engine.mito]
num_workers = 2
global_write_buffer_size = "64MB"
global_write_buffer_reject_size = "128MB"
CONFIG
start_engine() {
  "$engine_binary" standalone start --config-file "$engine_trial/config.toml" >> "$engine_trial/engine.log" 2>&1 &
  engine_pid=$!
  engine_probe=0
  until curl -fsS --max-time 1 -X POST --data-urlencode 'sql=SELECT 1' http://127.0.0.1:19116/v1/sql > "$engine_trial/ready.json" 2>/dev/null; do
    kill -0 "$engine_pid"
    engine_probe=$((engine_probe + 1))
    [ "$engine_probe" -lt 100 ]
    sleep 0.1
  done
}
export FABRIC_GREPTIME_ENDPOINT=http://127.0.0.1:19116
export FABRIC_GREPTIME_WITNESS="$engine_trial/witness.json"
start_engine
cargo test --locked -p fabric-server --test native_faults -- --ignored --exact unknown_inflight_write_and_gateway_death_preserve_pending_binding --nocapture
cargo test --locked -p fabric-server --lib -- --ignored --exact engine::tests::native_binary_custody_and_typed_oracle --nocapture
kill -KILL "$engine_pid"
wait "$engine_pid" 2>/dev/null || true
engine_pid=''
start_engine
cargo test --locked -p fabric-server --lib -- --ignored --exact engine::tests::native_restart_replays_binary_witness --nocapture
