#!/bin/sh
set -eu

kernel=$(uname -r)
cpu_model=$(awk -F ':[[:space:]]*' '/^model name[[:space:]]*:/ {print $2; exit}' /proc/cpuinfo)
memory_total=$(awk '/^MemTotal:/ {print $2; exit}' /proc/meminfo)
test -n "$cpu_model"
test -n "$memory_total"
printf 'Kernel %s\nCPU %s (%s available cores)\nTotal memory %s KiB\n' \
  "$kernel" "$cpu_model" "$(nproc)" "$memory_total"
printf 'CPU quota %s\nMemory limit %s\n' \
  "$(cat /sys/fs/cgroup/cpu.max)" "$(cat /sys/fs/cgroup/memory.max)"
rustc -vV
cargo -V
rpm -q gcc glibc openssl-devel
if [ "$1" = native-acceptance ]; then
  rustup run 1.99.1 rustc -vV
  rustup run nightly-2026-09-17 rustc -vV
  rpm -q gcc-c++
fi
