---
id: O-0002
title: Converged Release Verification Proofs, Fuzz Campaigns, and Coupled User-Stack Install
type: observation
status: observed
date: 2026-11-15
added_by: agent
human_ratified: false
related:
  - D-0001
  - D-0002
  - D-0003
  - O-0001
  - R-0002
---

# O-0002 · Converged Release Verification and Coupled Stack Installation

## Formal Verification & Fuzzing Observations (`develop`)

1. **Verus + Charon/Aeneas Lean 4 Extraction (`admission-kernel`, `proofs/ExtractedAdmission.lean`, `proofs/Telemetry.lean`, `proofs/TelemetrySmt.smt2`):**
   - Verified converged telemetry predicates (`gorilla_step_roundtrip`, `gorilla_bound_tight`, `rollup_bucket_aligned`, `rollup_bucket_monotone`, `ring_count_bounded`, `ring_byte_budget_bounded`, `watermark_monotone`, `watermark_advance_exact`) alongside `admission-kernel` Verus + Aeneas extraction and Z3 `QF_BV` proofs via `cargo xtask prove all`.
   - Documented `alignRollup` signed 64-bit clamping boundary (`[-9_223_372_036_854_600_000, i64::MAX]`, clamping extreme negative `i64::MIN` via `.saturating_mul`) so Lean 4 preconditions accurately mirror executable Rust arithmetic.
2. **Continuous Fuzzing (`fuzz/` — 8 targets including 3 converged targets):**
   - `unified_query_and_ipc` uncovered an upstream `arrow-ipc` `StreamReader::try_new` hazard: feeding arbitrary unpreflighted fuzz bytes treats a non-`0xFFFFFFFF` 4-byte continuation prefix as a legacy metadata length up to `~1.95 GiB`, exhausting ASan's `-rss_limit_mb=2048`.
   - Fixed the fuzz harness to fuzz SQL identifier/predicate/aggregation parsing (`validate_ident`, `parse_where_predicate`, `parse_agg_expr`, `build_unified_sql`), `DDSketch` bounds, and well-formed Arrow IPC roundtrips directly (keeping `arrow-ipc` off untrusted raw continuation bytes).

## Coupled Stack Installer (`xtask collection install-stack`)

- `xtask collection install-stack` couples `fabric-server.service`, `spindle.service`, and `fabric-daemon.service` inside `fabrico11y.slice` (`Type=notify` readiness ordering on `fabric-server` and `spindle`, graceful `SIGINT`/`SIGTERM` shutdown on `fabric-server` and `fabric-daemon` plus `ExecStop=spindle stop` graceful shutdown on `spindle.service`, and zero `PrivateTmp` or `ProtectSystem=strict` to avoid `226/NAMESPACE` failures on rootless Fedora SELinux user managers), pre-binds Spindle `origin` + Server `authority` + disjoint `0600` tokens, enforces non-`/tmp` storage across both storage and XDG layout paths, preflights conflicts before touching custody state, and preserves both offline and live idempotence.
- Because `fabrico11y.slice` omits `[Install]`, `fabric-server.service`, `spindle.service`, and `fabric-daemon.service` omit `After=default.target` to avoid an ordering cycle with `WantedBy=default.target`; activation runs `systemctl --user enable --now fabric-server.service spindle.service fabric-daemon.service`, while leaving any separate legacy Quadlets (`olo-fabric-*`) untouched unless explicitly deprecated by the operator.
