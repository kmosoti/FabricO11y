#!/usr/bin/env bash
set -euo pipefail

model_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
java_bin=${JAVA_BIN:-java}
tla_jar=${TLA_JAR:?Set TLA_JAR to the absolute path of tla2tools.jar}

if [[ ! -f "$tla_jar" ]]; then
    printf 'TLA_JAR does not exist: %s\n' "$tla_jar" >&2
    exit 2
fi

check_dir=$(mktemp -d)
trap 'rm -rf -- "$check_dir"' EXIT
cd "$model_dir"

run_tlc() {
    local name=$1
    local config=$2
    "$java_bin" -XX:+UseParallelGC -cp "$tla_jar" tlc2.TLC \
        -deadlock -workers 1 -fp 0 -metadir "$check_dir/$name" \
        -config "$config" DeliveryOwnership.tla > "$check_dir/$name.log" 2>&1
}

if ! run_tlc safe Safe.cfg; then
    cat "$check_dir/safe.log" >&2
    printf 'Safe model failed.\n' >&2
    exit 1
fi
if ! grep -Fq 'Model checking completed. No error has been found.' "$check_dir/safe.log"; then
    cat "$check_dir/safe.log" >&2
    printf 'Safe model did not report a complete check.\n' >&2
    exit 1
fi
if ! grep -Fxq '225 states generated, 64 distinct states found, 0 states left on queue.' "$check_dir/safe.log"; then
    cat "$check_dir/safe.log" >&2
    printf 'Safe model explored a different state space; review the model and bound.\n' >&2
    exit 1
fi
printf 'Safe model: all reachable states checked; no invariant violation.\n'

for case in 'early:EarlyAck.cfg:AckSafety' 'lost:LostCopy.cfg:RetainedOrDurable'; do
    IFS=: read -r name config invariant <<< "$case"
    set +e
    run_tlc "$name" "$config"
    tlc_status=$?
    set -e
    if [[ "$tlc_status" -ne 12 ]]; then
        cat "$check_dir/$name.log" >&2
        printf '%s returned %s; expected TLC invariant-failure status 12.\n' "$config" "$tlc_status" >&2
        exit 1
    fi
    if ! grep -Fq "Error: Invariant $invariant is violated." "$check_dir/$name.log"; then
        cat "$check_dir/$name.log" >&2
        printf '%s failed for a reason other than the expected invariant.\n' "$config" >&2
        exit 1
    fi
    expected_step='State 3: <EarlyAcknowledge'
    if [[ "$name" == lost ]]; then
        expected_step='State 4: <Forget'
    fi
    if ! grep -Fq "$expected_step" "$check_dir/$name.log"; then
        cat "$check_dir/$name.log" >&2
        printf '%s produced a different counterexample trace.\n' "$config" >&2
        exit 1
    fi
    printf '%s: expected %s counterexample found.\n' "$config" "$invariant"
done
