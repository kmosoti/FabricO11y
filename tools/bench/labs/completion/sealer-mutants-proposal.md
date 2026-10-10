# Proposed bounded-sealer mutant detectors

Status: patch prepared; not applied, registered, built or executed. The live
completion fixture and frozen acceptance binaries are unchanged. Root must
finish the live comparison before applying
[sealer-mutants.patch](sealer-mutants.patch).

Apply the example test separately from the verification policy. The patch has
two file sections so these operations can be staged in separate commits:

```sh
git apply --include=crates/fabric-server/examples/completion_builder.rs tools/bench/labs/completion/sealer-mutants.patch
git apply --include=xtask/mutants.json tools/bench/labs/completion/sealer-mutants.patch
```

The second section is a trust-boundary policy change: register the three
previously named milestone controls without weakening existing checks. Review
and commit its reason separately from the example test. A future source edit
can make an exact replacement stale; the runner must report that honestly.

| Control | Representative defect | Deciding existing/new test |
| --- | --- | --- |
| M-SEAL-MERGE-ORDER | Reverse each spill run's sort | Existing generated merge property uses a separately spelled contract key and stable sort, checking exact rows and tie payload order across byte limits. |
| M-SEAL-RETAIN-RUNS | Forget every successfully written owned spill row | New counted example test builds the registered steady 128 MiB fixture and checks actual incremental allocator peak against the unchanged 80 MiB ceiling. |
| M-SEAL-SPILL-LEFT | Skip failed-building-directory removal | Existing deterministic corruption-after-spilling test checks source custody and absence of scratch before any retry cleanup. |

The new test is compiled only with `responsibility-alloc-probe` and runs alone.
It uses the existing fixture generator and existing `run` function, which resets
allocator snapshots after generation and captures build peak before readback.
Historical steady-128 has 220370 logs: their 512-byte bodies alone occupy
112829440 bytes (107.6 MiB), exceeding the 80 MiB ceiling if retained. This is
a predicted detector outcome; historical unmutated incremental peak was
41673485 bytes. A new positive control must establish its own result.

Scratch is a fresh owned directory directly under `FABRIC_SCRATCH_ROOT`, with
no fallback. The compact JSON receipt records seed, exact fixture hash, actual
row count, allocator measurements, executable hash, leftovers and cleanup.
It is written under
`CARGO_TARGET_DIR/verification/sealer-heap-mutant/steady-128-PID.json` before
owned fixture cleanup and updated afterwards. No telemetry body is dumped on
failure. Archive this receipt with the mutant output before cleaning the
semantic-mutant work tree; the seed/hash preserve the replayable counterexample
without retaining a 128 MiB fixture. Early generation/build errors retain their
owned scratch for inspection.

Root serializes all commands through the ordinary resource launcher. The
registry commands invoke filtered libtests; the mutation runner requires a
passing unmutated baseline and the named `FAILED` result for the defective copy.
It changes neither the live source nor the frozen acceptance binary. Run each
new ID with `cargo xtask mutants --only ID` under containment and record the
actual status, output and receipts. A successful detector demonstrates sensitivity
to one meaningful defect; it does not complete all shape/scale BS-3 acceptance
or establish a universal heap bound.
