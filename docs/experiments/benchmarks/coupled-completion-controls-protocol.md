# Coupled candidate and availability controls

Registered before executing the new controls. The owner accepted completion
implementation. These are finite controls, not performance or qualification.
Existing production defaults remain serial/default16MiB; candidate APIs/selectors
are explicit experiments. All commands use resource_group + run_job +
coupled_admit with4MiB global reserve, zero capacity reserve;180s per job.
Relevant source is frozen during each job and captured in its coordinator archive.

- `catalog-coupled-o8-controls-01`, recovery stage:
  `cargo test --offline --locked -p fabric_o11y --lib overlap_ -- --nocapture`.
  Six deterministic controls cover durable successor before ACK, exact retry,
  no third Batch, full Spool/cursors, stop/reopen, future ACK, worker panic,
  paused collection/config, metering and rate-deadline refusal. Worker send is a
  private deterministic function; real TLS behavior is a later pilot requirement.
- `catalog-coupled-o6-integrity-01`, recovery stage:
  `env FABRIC_STORAGE_EVIDENCE=<ABSOLUTE_REPO>/docs/experiments/benchmarks/data/catalog-coupled-availability-01/integrity cargo test --offline --locked -p fabric-server --test completion_storage --all-features raw_batch_table_integrity_failures_are_incomplete -- --exact --nocapture`.
  Four fresh cuts: truncation, same-size footer magic damage, wrong-schema valid
  Parquet, incorrect manifest row count. Independent companion grades24 chains.
- `catalog-coupled-o6-source-errors-01`, recovery stage: same storage command and
  absolute evidence root `/source-errors`, selector
  `missing_gap_table_is_incomplete_and_missing_manifest_is_source_error`.
  Gaps missing must be visibly incomplete; Manifest missing must return bounded
  source movement error. This does not specify unknown metadata in those cases.
- `catalog-coupled-c5-selector-{8,16,32}-01`, preparation stage:
  `env FABRIC_RUN_MIB_EXPERIMENT=<VALUE> cargo test --offline --locked -p fabric-server --lib experimental_run_limit -- --nocapture`.
  Each selected build tests8/16/32 values, invalid parsing, byte-driven spilling,
  and the explicit oversized-row exception. This is not a whole-heap bound or
  proof that service output is identical at every setting; native comparison
  still needs frozen variants and an independently graded fixture.

Failure means preserve fixture/receipt, investigate, and use a fresh run ID after
correction. Do not infer end-to-end service readiness from these unit controls.
