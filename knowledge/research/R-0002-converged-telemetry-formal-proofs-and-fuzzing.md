---
id: R-0002
title: Formal Proofs and Fuzz Verification for Converged Telemetry Predicates
type: research
status: preliminary
date: 2026-11-15
added_by: agent
human_ratified: false
related:
  - R-0001
  - D-0002
  - O-0001
---

# R-0002 · Formal Proofs & Fuzz Verification for Converged Telemetry Predicates

## Verification Coverage Split

1. **Verus & Extracted Lean 4 (`admission-kernel::telemetry` + `proofs/ExtractedAdmission.lean`):**
   - `watermark_advance` enforces monotonic committed watermark progression on successful flushes and zero advancement on failed flushes (`watermark_advance_contract`).
   - `ring_can_buffer` enforces a soft half-ring cap (minimum `2048 B`) minus `25%` headroom (minimum `512 B`), ensuring unflushed buffering never crosses the half-ring limit (`ring_can_buffer_contract`).
   - `disk_needs_eviction` and `rss_shedding_decision` enforce exact boundary comparisons for ZSTD Parquet quota eviction (`RetentionJanitor`) and self-preservation RSS shedding (`SelfPreservationGuard`), proven in both Verus (`--no-cheating`) and Charon/Aeneas-extracted Lean 4 without custom axioms.
2. **Z3 Bit-Vector SMT (`proofs/TelemetrySmt.smt2` via `xtask prove smt`):**
   - Verifies four `QF_BV` properties over 64-bit bit-vectors: ZigZag `i64`/`u64` bijection, modular two's-complement Gorilla timestamp delta-of-delta reconstruction across negative and signed-wrap deltas, zero-trimmed IEEE-754 `f64` XOR roundtrip fidelity (including `-0.0` and distinct NaN bit payloads), and `watermark_advance` monotonicity.
   - Includes a semantic negative control mutating modular `bvsub` into saturating subtraction (`ite (bvuge t1 t0) ... 0`), rejected as `sat`.
3. **Standalone Lean 4 (`proofs/Telemetry.lean` via `xtask prove telemetry`):**
   - Proves 9 theorems covering Gorilla bit-level delta-of-delta step and inductive stream roundtripping (`gorilla_step_roundtrip`, `gorilla_stream_roundtrip`), Euclidean 5-minute (`300_000 ms`) rollup bucketing (`rollup_bucket_aligned`, `rollup_bucket_residual_bounds`, `rollup_bucket_idempotent`, `rollup_bucket_monotone`), and multi-step watermark monotonicity (`watermark_failed_flush_unchanged`, `watermark_step_monotone`, `watermark_trace_monotone`).
   - Includes a semantic negative control mutating failed flush handling to advance prematurely, rejected by Lean 4.
4. **Continuous Coverage-Guided Fuzzing (`fuzz/`):**
   - Adds `gorilla_ring_roundtrip`, `unified_query_and_ipc`, and `causal_graph_and_wire` alongside the 5 core custody fuzz targets (`8` total targets replayed in CI and verified via `xtask fuzz replay`).

## Relations

- *builds-on*: [R-0001](R-0001-converged-rust-storage-and-five-pillars.md)
- *supports*: [D-0002](../decisions/D-0002-cli-pivot-and-converged-telemetry-engine.md)
- *grounded-in*: [O-0001](../observations/O-0001-fedora-blade-resource-postmortem.md)
