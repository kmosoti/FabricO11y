# Verification receipt representation

This is the validation policy for the Rust receipt verifier, not a claim that
historical receipts were revalidated.
The owner authorized tooling/verifier migration and stronger quality controls.
The Python verifier is retained as historical reference evidence, not as a
maintained runtime dependency. Its source at immutable Git commit
[`f001175ff4006e2f95294458ce36f4f089d3be7d`](https://github.com/kmosoti/FabricO11y/tree/f001175ff4006e2f95294458ce36f4f089d3be7d)
has SHA-256 `912ede91d27277bb0e45a337d1266ec1ea191b1029a536ca9f1266ced3559a23`.
The independent expected outcomes for the maintained Rust CLI live in
[`xtask/tests/fixtures/receipt_contract_cases.json`](../../xtask/tests/fixtures/receipt_contract_cases.json);
the fixture is authored from this contract, not copied or generated from either
validator. Historical Python-vs-Rust reports remain historical evidence and
are not current-source acceptance evidence.

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
reader. The historical Python baseline accepted duplicate result keys,
boolean versions, floating-point versions, and numeric dirty-state aliases;
the maintained fixture expects the Rust checker to reject each one under this
explicit policy. It prevents interpretation from depending on key order or
language equality coercions. The current Rust receipt writer already emits integer
versions and boolean dirty state. No expected negative outcome, product oracle,
required check, proof bound, timing exposure or acceptance condition is relaxed.

The verifier establishes receipt integrity/completeness under filesystem and
SHA-256 assumptions. It cannot attest that an unsigned synthetic receipt's
command ran, establish arbitrary-program correctness or confer certification.
Identity rechecks assume exclusively owned inputs and honest kernel metadata;
they detect observable changes, not every possible metadata-restoring attack.
They do not create an immutable filesystem snapshot or protect a compromised
host. The digest attests the admitted bytes actually read. Existing mutation
and replacement controls remain required and must keep their expected refusals.
Current production adoption requires the fixture-driven Rust CLI controls, the
representative defect canary, and relevant source/workflow checks to run. The
historical Python differential remains evidence for its recorded revisions,
not a required current-tree runtime or current-source acceptance result. A
policy document is not evidence that those checks ran.
