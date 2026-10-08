# Verification after test-module placement correction

The [ownership transition investigation](catalog-transition-ownership-protocol.md)
completed its targeted checks. `catalog-transition-checks-01` then ran all 17 fast
gates: workspace tests and 15 other gates passed, but Clippy rejected an inline
snapshot test module preceding production items (`items_after_test_module`).
The fast profile exited 1; manual documentation checks did not run. Preserve
that outcome, the exact source archive and all receipts unchanged.

Move the same test body into `query/snapshot_evidence_tests.rs`, with an external
test-module declaration. Change no assertion, negative-control expectation or
production behavior. Format inside the resource group before freezing sources.

Register one new `catalog-transition-checks-02` verification job with 600 seconds,
one MiB reserve after coordinator snapshots and 512 KiB driver output cap. It
runs the fast and manual documentation profiles, exact source hash checks and
the existing Bun archive-member cleanup. Dispatch the existing transition driver
with `--mode checks --checks-attempt 2`. All existing frontier/stage/evidence and
20 GiB high/max/no-swap containment rules apply unchanged (memory high remains
16 GiB; max remains 20 GiB). This does not reset any time allocation.

Before checks, preserve all remaining files from the previous launcher's owned
failure tree `fabric-work-e051d6137a624086836988a3644fd7fa` on the data drive.
Require that transient unit to be inactive. The observed tree holds two small
successful fuzz-corpus scratch files and their lock files; allow at most 512 KiB
decoded data. Archive every file and compare complete coverage, lengths and exact
decoded bytes with the originals before removing that owned tree. Record its
original path, archive and readback in the new receipt. Keep the original failure
receipts and counterexample archives. If preservation or the bound fails, retain
the tree and report it rather than weakening cleanup or evidence requirements.

Final documentation distinguishes the failed first full verification from the
new run's actual results. A module relocation is not a new performance experiment.
