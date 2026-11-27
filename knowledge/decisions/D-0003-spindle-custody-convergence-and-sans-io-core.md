# D-0003 · Spindle Custody Convergence and Sans-I/O Core Extraction

## Decision

Unify SDK/Spindle/canonical custody telemetry with the converged OLAP store while deepening Sans-I/O verification boundaries:

1. **Sans-I/O Extraction (`fabric-core`):**
   - Move `HotMetricRing` (modular `wrapping` Gorilla delta-of-delta + XOR encoding, label-preserving `SeriesKey`, `align_rollup_bucket_ms`, `restore_unflushed`) into `fabric_core::ring`.
   - Move `UnifiedQuerySpec`, identifier validation, leftmost-operator predicate parsing (quoting non-numeric strings and non-finite floats), and aggregate SQL generation into `fabric_core::query_spec`.
   - Keep `fabric-store::ring` and `fabric-query::unified` focused strictly on disk Parquet/redb/DataFusion I/O while re-exporting pure types.
2. **Spindle Custody Bridge & Watermark Durability (`fabric-collect`):**
   - `fabric-collect` (`COL --> FAB`) polls versioned custody records from `spindle.sock` (`poll_spindle_custody_records`) within a bounded `250ms` per-tick budget alongside non-blocking `ingest.sock` JSONL/wire intake.
   - Maintain an in-memory `polled_spindle_cursor` and `pending_spindle_watermark`; persist `spindle_watermark_id` in `CatalogStore` (`redb`) **only after** `flush_to_olap` completes all five OLAP Parquet writes without error.
   - On queue saturation (`pending_logs`/`pending_traces`/unflushed ring budget), halt further Spindle polling for that tick without advancing the cursor past unaccepted records and trigger an immediate backpressure flush.
3. **Deterministic Custody Inspection (`fabric custody`):**
   - `fabric-cli` (`CLI --> FAB`) adds `fabric custody [--json | --arrow | --table]` to inspect local Spindle socket health, forwarding progress, `fabric-server` canonical frontiers, collector `spindle_watermark_id`, and explicit `spindle_error`/`server_error`/`catalog_error` diagnostics.
4. **Non-`/tmp` Workspace & CI Hygiene:**
   - Replace direct `/tmp` scratch assumptions and stale toolchain pins across `xtask`, `.github/scripts/install-verification-tools.sh`, and CI workflows with `non_tmp_base_dir()` routing and `nightly-2026-09-17` alignment.

## Alternatives Considered

- **Advancing `spindle_watermark_id` immediately upon socket poll:** Rejected because a process exit or disk write failure before the 60s Parquet flush would permanently skip uncommitted Spindle records; buffer rollback + post-flush watermark commit makes custody-to-OLAP ingestion crash-safe at-least-once.
- **Keep `HotMetricRing` and SQL predicate builders inside I/O crates:** Rejected because pure algorithms belong in `fabric-core` for direct deterministic unit testing, Z3/Lean proof alignment, and zero-dependency fuzzing.

## Revisit When

- If `fabric-collect` moves from pull-based Spindle socket polling to push-based shared-memory ring notifications, preserve the post-Parquet watermark advancement invariant.

## Relations

- *extends*: [D-0002](D-0002-cli-pivot-and-converged-telemetry-engine.md)
- *backed-by*: [R-0001](../research/R-0001-converged-rust-storage-and-five-pillars.md)
