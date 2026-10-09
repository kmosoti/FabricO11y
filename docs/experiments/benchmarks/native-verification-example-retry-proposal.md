# Native verification example repair and second failure preservation

The [first retry](native-verification-retry-proposal.md), `final-fast-02`, again
passed all workspace tests and sixteen checks. Clippy found the same redundant
outer OTLP request update in `examples/key_first_query_probe.rs:226`, which its
earlier failed target had not reported. Remove that redundant initializer only.
Do not change generated rows, query logic, thresholds or assertions. Frozen
native binaries and the original source archives remain the performance evidence;
no measurements are rerun to replace the adverse results.

Run the unchanged full Clippy command as a contained preflight before another
full fast profile. A fresh `final-fast-03` then runs all seventeen unchanged fast
checks with two Cargo build jobs and two Rust test threads, at most 900 seconds.
Charge preflight, retries and cleanup to the existing round. Do not overwrite
either failed receipt. The second failure's cgroup peak was 610,545,664 bytes,
with zero swap and high/max/OOM events; this is resource observation, not a matched
test-run performance comparison.

Extend the not-yet-executed preservation helper's exact allowlist with one pair:
`final-fast-02` maps only to unit
`fabric-work-21ec85a38b874063863ef6327431e027`. Preserve that unit's retained outer
tree independently using its own native/resource receipts and a fresh job/output.
The first pair remains exact and available. All earlier inactivity, source
snapshot, 64 MiB raw/8 MiB archive, negative-control, two-readback and unchanged
inventory requirements remain. Each preservation job has at most 60 seconds.
No other evidence root may be selected; an unknown job must be rejected. Preserve
both registered supplements with the helper. Cleanup receipts append evidence
and never change the historical failed `scratch_removed` flags.
