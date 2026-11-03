# Canonical custody and investigation

New collection supports a context-bearing schema revision alongside byte-exact v2
records. A global run identity joins observations across processes and origins;
historical local run names never imply global correlation. Schema descriptors are
immutable and retained. SDK memory acceptance, local custody and Fabric custody
are distinct results. Recording never blocks on registration, disk or transport.

Compare an engine-led GreptimeDB 1.2.1 candidate with an existing-SQLite-led
canonical receiver and Arrow/Parquet/DataFusion analytical projection. Every
candidate must preserve original bytes, full unsigned integer domains and finite
float bits; authenticate origin; reject conflicts without overwrite; recover
acknowledged records; expose faithful replay; and confirm only contiguous prefixes.
Incorrect or unsupported candidates cannot enter performance ranking. If no
candidate qualifies, migration stops. Correctness fixtures and the evaluator are
frozen before candidate refinement; held-out fixtures open only after selection.

Fabric is the authoritative retained copy after handoff. Every Spindle has a
persistent origin and monotonically allocated record IDs, including after payload
reclamation and restart. A Fabric receipt names the configured authority, origin
and confirmed prefix. Local payload reclamation requires that matching durable
confirmation. Lost/wrong/stale acknowledgements never discharge custody. Read and
export must resolve reclaimed payloads or report unavailability, never false EOF.
Quota accounting includes recovery reserve, metadata and reusable database pages.

Context-aware clients use authenticated generation leases, normally 24 hours.
Expiry remains detectable without retained generation metadata. Expired leases
refuse new admission/retries; previously sealed pending records remain retained
and forwardable. Renewal requires a quiescent client. Clocks that move behind a
persisted floor fail closed for lease decisions. Per-generation ordered sequences
and bounded generation count permit compact retry frontiers. Reclaimed retries
are checked against canonical bytes or return unknown/unavailable. V2 retains its
original retry semantics and is not retroactively expired.

Queries expose typed run listings, run/activity summaries, time buckets and
supporting evidence through one Rust/CLI/MCP contract. Source authorization precedes
aggregation. Each answer binds source scope, schema/transform identity and a fixed
vector of contiguous source prefixes. Pagination cannot change that snapshot.
Projection lag, observed gaps, budgets, truncation and coverage are explicit.
Budget-exhausted aggregate scans cannot be reported as complete. Wide integers
remain exact in the agent protocol. Cancellation retains reservations until actual
worker cleanup. No free SQL, file access or observed-text instructions are exposed.

Acceptance uses one million mixed observations and four isolated G15 Spindles
feeding Blade, plus an ordinary service and a cross-process agent harness. Offered
load is 1,000 records/s; drainage completes within 30 seconds after load stops;
selective run/time queries complete within two seconds. Qualified candidates are
compared by full receiver-group memory across five repetitions; nonoverlapping
lower-memory results win, otherwise retain the SQLite baseline. New held-out
workloads qualify the selected candidate rather than selecting it again.

Build with two jobs on Blade. G15 test workloads share 2 GiB with no swap; receiver
workloads share 4 GiB. Query defaults are one active worker, 256 MiB execution pool,
512 MiB spill, ten seconds and bounded result size. Measure process/group memory
separately from allocator/pool accounting. Native kernel proofs, adapter tests,
fault injection and measurements have separate stated scopes. Exact source,
configuration, tools and named checks bind acceptance receipts.

All trials use owned private state and credentials. Existing Fabric/node services
are not trial targets. Canonical history is not automatically expired. Backfill and
shadow comparison precede enabling reclamation; rollback preserves canonical access
to reclaimed records. Physical power-loss durability and third-party database
implementation proofs remain outside the evidence established by process tests.
