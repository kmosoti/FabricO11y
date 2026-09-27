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
        -config "$config" TransportOwnership.tla > "$check_dir/$name.log" 2>&1
}

if run_tlc safe Safe.cfg; then
    tlc_status=0
else
    tlc_status=$?
    cat "$check_dir/safe.log" >&2
    printf 'Safe.cfg returned %s; expected 0.\n' "$tlc_status" >&2
    exit 1
fi
if ! grep -Fq 'Model checking completed. No error has been found.' "$check_dir/safe.log"; then
    cat "$check_dir/safe.log" >&2
    printf 'Safe.cfg did not report a complete check.\n' >&2
    exit 1
fi
if ! grep -Fxq '251 states generated, 130 distinct states found, 0 states left on queue.' "$check_dir/safe.log"; then
    cat "$check_dir/safe.log" >&2
    printf 'Safe.cfg explored a different state space; review the model and bound.\n' >&2
    exit 1
fi
printf 'Safe.cfg: TLC exit %s; 130 distinct states, all invariants hold.\n' "$tlc_status"

check_expected_trace() {
    local name=$1
    local config=$2
    local invariant=$3
    shift 3

    if run_tlc "$name" "$config"; then
        tlc_status=0
    else
        tlc_status=$?
    fi
    if [[ "$tlc_status" -ne 12 ]]; then
        cat "$check_dir/$name.log" >&2
        printf '%s returned %s; expected TLC invariant-failure status 12.\n' "$config" "$tlc_status" >&2
        exit 1
    fi
    if ! grep -Fq "Error: Invariant $invariant is violated." "$check_dir/$name.log"; then
        cat "$check_dir/$name.log" >&2
        printf '%s failed for a reason other than %s.\n' "$config" "$invariant" >&2
        exit 1
    fi
    local step
    for step in "$@"; do
        if ! grep -Fq "$step" "$check_dir/$name.log"; then
            cat "$check_dir/$name.log" >&2
            printf '%s did not contain expected trace step %s.\n' "$config" "$step" >&2
            exit 1
        fi
    done
    printf '%s: TLC exit %s; expected %s trace found.\n' "$config" "$tlc_status" "$invariant"
}

check_expected_trace early EarlyAck.cfg AckSafety \
    'State 5: <EarlyAcknowledge'
check_expected_trace overgrant OverGrant.cfg CreditBound \
    'State 4: <SendFresh' 'State 5: <OverGrantCredit'
overgrant_state=$(awk '
    /^State 5: <OverGrantCredit/ { found = 1; next }
    found && (/^State [0-9]+:/ || /^[0-9]+ states generated/) { exit }
    found { print }
' "$check_dir/overgrant.log")
if ! grep -Fxq '/\ credits = 2' <<< "$overgrant_state" ||
   ! grep -Fxq '/\ inFlight = {<<p1, 1>>}' <<< "$overgrant_state"; then
    cat "$check_dir/overgrant.log" >&2
    printf 'OverGrant.cfg did not show 2 credits plus 1 scheduled packet against budget 2.\n' >&2
    exit 1
fi
check_expected_trace retry RetryWitness.cfg NoDedupResponse \
    'State 7: <LoseAck' 'State 8: <SendRetry' \
    'State 9: <RespondDuplicate'
