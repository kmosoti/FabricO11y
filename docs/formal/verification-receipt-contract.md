# Verification receipt representation

This is a prospective validation policy for the Rust receipt verifier, not a
claim that it is implemented or that historical receipts were revalidated.
The owner authorized tooling/verifier migration and stronger quality controls.
The original Python verifier remains unchanged as an independent reference.

The baseline at clean source `fa5fc02acdffcefcf9dff424b9297d20ee5c8899`
completed with command/outer exit 0 and confirmed stop. Its synthetic-fixture
report `results/rust-tooling-migration/receipt-baseline-01/baseline.json` has
SHA-256 `cae8eec26c7306aa9bdbcaa77da2038599e76894c57de66b66973917e5dbc0e3`.
Twenty ordinary negative controls were rejected; four separate probes were
accepted: duplicate result keys, boolean version, floating-point version and
numeric dirty state. This is structural-validator evidence, not execution of
the commands described by the synthetic receipts.

## Representation rules

- JSON objects have unique keys, finite numbers and at most 32 container levels.
  Registry and receipt documents are at most 4 MiB each; aggregate retained JSON
  follows the existing artifact-reader budget. Unknown descriptive fields remain
  allowed; they cannot override required fields.
- `receipt_version` is the JSON integer 1. `worktree_dirty` is JSON false.
  Boolean/numeric aliases and floating-point versions are rejected.
- `exit_code` is the JSON integer 0. `duration_ms` is a nonnegative JSON integer
  representable as `u64`; booleans and floating-point values are rejected.
- The current clean source, selected profile, check ID, original command array,
  passed result and canonical log filename must match exactly. Registry IDs are
  unique and use the existing lowercase identifier grammar. Command arguments
  are strings; no lossy filtering or coercion is permitted.
- SHA-256 is 64 lowercase hexadecimal characters and matches all admitted log
  bytes. Receipt/log leaves and their paths below the admitted root must be
  ordinary files/directories, with descriptor-relative no-follow access.
  Replacement or mutation before, during or after reading is a failure.
- Any current-check directory entry, including a dangling symlink, prevents
  success. Missing, malformed, failed, interrupted or unavailable evidence is
  never a successful profile. Read budgets and arithmetic fail closed.

Rejecting the four observed representations deliberately strengthens the legacy
reader. It prevents interpretation from depending on key order or language
equality coercions. The current Rust receipt writer already emits integer
versions and boolean dirty state. No expected negative outcome, product oracle,
required check, proof bound, timing exposure or acceptance condition is relaxed.

The verifier establishes receipt integrity/completeness under filesystem and
SHA-256 assumptions. It cannot attest that an unsigned synthetic receipt's
command ran, establish arbitrary-program correctness or confer certification.
Production adoption requires the registered differential/failure experiments
and relevant source checks; a policy document is not their result.
