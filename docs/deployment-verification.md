# Deployment verification

Choose an exact source candidate and build the release binaries from that
candidate. The executable fixture uses its own services, stores, and credentials.
It leaves the installed stack running as configured.

## Confined executable fixture

Run this on a Linux host with the repository's
[`fabrico11y.slice`](../config/fabrico11y.slice) installed in the user manager.
Use Bash for the commands below. The fixture runs `spindle`, `fabric-server`,
`fabric`, and `fabric-adapter`. Set `FABRIC_TEST_BIN_DIR` to the directory
containing the packaged or previously built executables from the same source
candidate to reuse them.

```sh
set -o pipefail
export TMPDIR=/data/tmp CARGO_BUILD_TARGET_DIR=/data/cargo-target
export FABRIC_DATA_DIR=/data/fabrico11y
if [[ -z "${FABRIC_TEST_BIN_DIR:-}" ]]; then
  cargo +1.100.0 build --locked --release -p fabric -p fabric-cli -p fabric-server \
    --bin fabric --bin fabric-adapter --bin spindle --bin fabric-server -j 2
  FABRIC_TEST_BIN_DIR=$CARGO_BUILD_TARGET_DIR/release
fi
test_bin=$(
  cargo +1.100.0 test --locked -j 2 -p xtask --features deployment-qualification \
    --test deployment_profiles --no-run \
    --target-dir /data/cargo-target --message-format=json |
    jq -er 'select(.reason == "compiler-artifact" and .target.name == "deployment_profiles" and .executable != null) | .executable'
)
sha256sum "$test_bin" "$FABRIC_TEST_BIN_DIR"/{fabric,spindle,fabric-server,fabric-adapter}
systemd-run --user --collect --wait --pipe --slice=fabrico11y.slice \
  --setenv=TMPDIR=/data/tmp --setenv=FABRIC_DATA_DIR=/data/fabrico11y \
  --setenv="FABRIC_TEST_BIN_DIR=$FABRIC_TEST_BIN_DIR" \
  timeout 300 "$test_bin" --ignored --nocapture --test-threads=1
```

The fixture checks these observable boundaries.

| Scenario | Acceptance |
| --- | --- |
| Disconnected single host | A durable receipt returns `pending` before the local projection and `visible` after the exact row commits. |
| Two originating workloads | Distinct Spindle stores admit overlapping IDs and producer identities. A credential for one origin cannot read the other. The receiver returns exact source bytes at a fixed snapshot. |
| Receiver failure and recovery | Cut the receiver process, accept and query another observation locally, and observe canonical unavailability. Restart against the same custody and projection directories. The authority remains stable and the new record becomes visible exactly once. |
| Self-observability | A separate telemetry origin collects both services' metrics, events, logs, and traces. Compare the receiver's exact raw rows with the local durable prefix and query metrics, logs, and traces through the local projection. |
| Batch replay | Spindle commits a report and the fixture withholds its receipt. Kill and reopen the adapter, recover the original receipt, then verify that 130 distinct runs share one generation and project with exact bytes and no duplicates after the receiver restarts. |

A passing fixture emits `passed: true`, the authority and origins, the canonical
telemetry frontier, and elapsed time. Record the source revision, hashes, and
systemd slice during the run. Elapsed time includes waiting for periodic service
telemetry. It is not a throughput comparison.

## Physical multi-host run

Use the [multi-host setup](canonical-custody.md#connect-multiple-hosts) to deploy
two origin hosts and a receiver. The receiver can share a host with one origin.
Keep shadow forwarding enabled throughout an outage test. Record the exact
candidate, executable hashes, host roles, loopback endpoints, filesystem, and
slice limits. Preserve credential file paths or fingerprints in the report only
when needed to distinguish grants. Omit credential contents.

1. Enroll the two workload origins and the telemetry origins. Verify that host
   grants are distinct and that telemetry origins use the separate configured
   canonical quotas. Configure the server to exclude telemetry origins from
   its workload self-instrumentation. Exercise the same receipt and query paths
   as the fixture with the actual remote receiver endpoint behind each tunnel.
2. Stop the tunnel for one workload origin at its source host. Leave the second
   origin connected. Submit a new synthetic workload observation on each host
   and save its original wire receipt. Check the disconnected receipt using
   local `fabric projection-ready`. Record the canonical frontier of each
   origin and the coverage of an explicit `fabric-query` snapshot. Query
   completeness is scoped to that snapshot. The receiver cannot account for
   subsequent custody on a disconnected host.
3. Restore the tunnel. Check the recovered receipt with
   `fabric projection-ready --target canonical` and compare the observation
   with the receiver's query evidence. Verify one committed row for the exact
   origin and source ID. Compare the receiver's progress with each host's
   reported custody to confirm that the unaffected origin continued to advance.
4. Trigger a workload commit and query while both services emit their periodic
   metrics and traces. Read a bounded, fixed telemetry prefix locally and
   query evidence for the same prefix centrally. Identify each service's
   metric, event, log, and trace by its producer and observation type. Compare
   original bytes. Then leave only telemetry delivery active for a reporting
   interval and verify that telemetry-only work produces no workload operation
   traces. Record drops, blocked forwarding, peak observed RSS, open FDs, and
   storage growth for the selected run.

The executable fixture uses one host and a loopback receiver. Its process
restart exercises real custody and replay, while physical tunnel behavior,
different host clocks and kernels, and power-loss durability require their own
evidence. Lean frontier properties cover the pure commit decision. Deployed
transport, authorization, storage behavior, and periodic self-instrumentation
remain runtime test obligations.
