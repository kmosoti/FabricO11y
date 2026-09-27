# Final conceptual check

Read current `IDEAS.md` and `KILL_TESTS.md` once. Both explicitly state that E1R/E2R/E3R supersede E1/E2/E3 before any run, and that the amended protocols and latest promotion notes govern earlier ledger wording. All registrations remain `not_run`; no implementation is assessed.

The four prior contradictions are addressed:

1. **I7:** `IDEAS.md` separates trusted builder rejection from row-free receipt verification. E1R expects a deliberately sealed semantic bug to pass the receipt verifier and fail the independent row oracle; `builder_version` is identification, not a correctness proof.
2. **I8 seal-only subset:** The mechanism and E2R require a full seal with missing frame content to fail closed and preserve bytes. The thought experiment no longer promises truncation for every partial subset.
3. **I8 failed sync:** Both documents require an external supervisor or caller to retain EIO knowledge. E2R tests identical file bytes under success and EIO, with different externally informed resume decisions; it does not claim a file-only detector.
4. **I9 coordinate conflict:** Both documents reject different values at the same coordinate and merge only equal bytes or digests. E3R injects this conflict and a first-value-wins mutation.

The proposals are **conceptually ready for falsification** under their stated trust and finite-model assumptions. Passing the registered tests would not establish physical durability, malicious-executor query proofs, or performance; the documents say this explicitly. No remaining contradiction in the four requested areas was found.

VERDICT: APPROVE
