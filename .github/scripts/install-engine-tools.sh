#!/bin/sh
# Provision the pinned native engine binary used by the integration checks.
set -eu
engine_tools="${FABRIC_ENGINE_TOOLS:-$HOME/.local/share/fabrico11y/engine-tools}"
engine_binary="${FABRIC_ENGINE_BINARY:-$engine_tools/greptime-linux-amd64-v1.2.1/greptime}"
if [ ! -x "$engine_binary" ]; then
    if [ -n "${FABRIC_ENGINE_BINARY:-}" ]; then
        printf 'Configured engine binary is unavailable: %s\n' "$engine_binary" >&2
        exit 1
    fi
    mkdir -p "$engine_tools"
    engine_install=$(mktemp -d "$engine_tools/install.XXXXXXXX")
    trap 'rm -rf "$engine_install"' EXIT HUP INT TERM
    curl --proto '=https' --tlsv1.2 --retry 3 -fLsS https://github.com/GreptimeTeam/greptimedb/releases/download/v1.2.1/greptime-linux-amd64-v1.2.1.tar.gz -o "$engine_install/greptime.tar.gz"
    printf '%s  %s\n' 062d5beb13de4991c4fec201b2e5071518bf4ca8f0d52947eba757a74c2a5420 "$engine_install/greptime.tar.gz" | sha256sum -c -
    tar -xzf "$engine_install/greptime.tar.gz" -C "$engine_install"
    mkdir -p "$engine_tools/greptime-linux-amd64-v1.2.1"
    install -m 755 "$engine_install/greptime-linux-amd64-v1.2.1/greptime" "$engine_tools/greptime-linux-amd64-v1.2.1/greptime"
fi
printf '%s  %s\n' a7ca4d5ee33bf7605dc69a7be46ae3ad34c5fba27d14edacf780af285dad179d "$engine_binary" | sha256sum -c -
"$engine_binary" --version | grep -Fx 'version: 1.2.1'
