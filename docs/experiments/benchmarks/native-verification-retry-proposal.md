# Native round verification retry and failure preservation

Registered after `final-fast-01` completed, before its retry. The initial fast
profile passed all workspace tests and sixteen of seventeen checks, but Clippy
rejected a redundant `..Default::default()` in the outer one-field OTLP request
initializer of `tests/key_first.rs`. Remove only that redundant initializer; keep
the corruption fixture, assertions and all measurement sources unchanged. This
is a fixture lint repair, not a new candidate or a replacement performance run.
Preserve the failed receipt and its source-bound native measurements.

The full workspace job peaked at 12,905,226,240 cgroup bytes, with no high/max/OOM
events or swap use. For the retry set `RUST_TEST_THREADS=2` alongside
`CARGO_BUILD_JOBS=2`, retaining all seventeen configured fast checks and their
unchanged commands. This reduces concurrent test memory demand; it is not a
performance comparison or a relaxed check. Admit at most 900 seconds inside the
unchanged 1,800-second outer service deadline and existing cumulative round
budget. Use a fresh `final-fast-02` job and receipts directory. Manual documentation
checks remain required. Neither this retry nor cleanup changes nomination rules.

The new closeout verifier accepts this exact additional environment prefix, and
still checks all seventeen fast plus three documentation receipts, actual job
commands, revision, exit status and execution windows. Add controls rejecting an
unexpected test-thread value and a reduced `--only` profile. This verification
change is committed separately with this reason; it does not change an oracle,
product invariant or expected check outcome.

Preserve the inactive failed outer tree
`/run/media/kmosoti/data/FabricO11y/evidence/fabric-work-c3a63f9ab40a477c9f3189eb298aee66`
before removing it. The resource receipt under `target/resource-containment/runs`
and the native `final-fast-01/receipt.json` identify its origin. A new narrowly
scoped helper may reuse the existing `sweep_preserve.py` inventory, archive,
negative controls and exact readback functions. It must validate this exact
unit/path/job binding and inactivity, cap raw bytes at 64 MiB and the archive at
8 MiB, preserve every regular file and directory, recheck inventory and two exact
archive readbacks, then remove only this authenticated owned tree. On any mismatch
retain the original. Keep the original failed receipt unchanged and append a
separate preservation receipt under the native coordinator category. Preserve the
helper sources and both origin receipts with hashes. Admit this job for at most
60 seconds with 8 MiB reserved evidence; no other historical evidence is reclaimed.

The final resource report must include the failed job's time and 12.0 GiB peak,
the actual retry peak, all cleanup outcomes and the extra documentation check
after report reconciliation. Build caches remain on the mounted data drive.
