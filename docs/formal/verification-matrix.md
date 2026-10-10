# Verification matrix

A map from each product claim to its implementation, checks and remaining limits. Status uses the [evidence states](../QUALIFICATION.md#evidence-states): "Tested" records named automated evidence on the revision or working-tree snapshot identified by its receipt, not a guarantee for every later edit. The [check registry](../../xtask/checks.json) determines which checks run in each profile; fresh fast-check receipts are written under `target/verification/receipts`. Historical records apply only to the sources they name. Semantic mutant IDs refer to [xtask/mutants.json](../../xtask/mutants.json); results are recorded in the [milestone record](../milestones/architecture-foundation.md#semantic-mutants).

Specification sources: **PC** = [product contract](../PRODUCT-CONTRACT.md), **ADR-n** = [decisions](../decisions/README.md), **RH** = [retained history](../architecture/retained-history.md), **DO**/**QO** = [delivery](../../tools/qualification/DELIVERY_ORACLE.md)/[query](../../tools/qualification/QUERY_ORACLE.md) oracle specifications.

## Identity and operator console

These are new release obligations, not inherited CTRL/HIST passes. Detailed
fixtures/negative controls must be registered before candidate acceptance.
The [identity design](../architecture/identity-access.md) and
[console plan](../milestones/operator-console.md) define their intended boundaries.

| ID | Contract | Planned verification and independent expectation | Required negative control | State |
| --- | --- | --- | --- | --- |
| ACCESS-1 | Local passkey ceremony, session and protected recovery identify the intended human | Fixed account/credential fixtures; packaged HTTPS and real authenticator/browser checks; replay, UV, origin/RP, bootstrap race, fixation, CSRF, key loss, logout and expiry | Accept wrong origin/replayed challenge or restored session | Implemented; 18 adapter tests and Chromium virtual-authenticator registration/login/logout checks passed. Physical authenticators and full browser matrix remain unqualified. |
| ACCESS-2 | Every read/control surface enforces current action and Spindle/signal scope | Disjoint principals, all signal oracles, inventory/evidence/page fixtures and barrier-controlled revoke races; no hidden identities or misleading scoped completeness | Leak one forbidden row/metadata field or accept another principal's cursor | Seven HTTP tests passed, including journal/Segment scope canaries, cursor ownership and publication barriers. Removing the final GET authority check made both barrier regressions fail. Expanded packaged-browser scope cells remain in progress. |
| ACCESS-3 | Workloads and delegated AI cannot expand authority | Fixed grant-intersection truth tables, expiry/rotation/audience and parent-revocation fixtures; Spindle credential crossover denied | Widen one delegation intersection or trust client actor/role | Adapter truth tables, rotation/expiry/audience/parent retirement and three scoped CLI tests passed. Packaged delegation workflows remain in progress. |
| ACCESS-4 | Access/audit persistence and restore fail closed while preserving custody | Crash/sync/rename faults, audit/state caps, offline restore and concurrent policy/mutation fixtures; pre-revoke Batch admission may finish | Publish stale authority or resurrect a revoked credential from backup | Adapter tests cover five durable-intent cuts, restore epochs, quarantine and bounded audit publication. Finite fixtures do not establish arbitrary crash/power-loss recovery; full candidate acceptance remains open. |
| UI-1 | UI answers preserve exact values, bounded work and evidence semantics | [Algorithm model](../architecture/console-algorithms.md), [native model tests](../../crates/fabric-ui/tests/model.rs), registered browser workloads and unchanged query/rate oracles | Accept stale session result, round nanosecond ordering or bridge a chart gap | Native UI tests: 25 passed; WASM check/build exited 0. Chromium HTTPS workflow trials exercise real producer fixtures. Full U1–U6 acceptance remains in progress. |
| UI-2 | PWA caches only public shell; installed lifecycle respects access and compatibility | U1–U6 packaged-browser checks, sensitive cache fixtures, offline/logout/account switch and upgrade/rollback | Cache an API response or reapply an old account's result | Chromium baseline passed 31 finite checks, including sensitive-cache inspection, offline clearing and cross-tab logout. Injected API caching failed the named cache assertion. Installed/update/mobile cells are not thereby qualified. |

## Supporting investigations

These finite records supplement the claim rows without changing oracle expectations or qualification status. Each record identifies its checked sources and limits.

| Claims | Evidence | Limits |
| --- | --- | --- |
| HIST-1/2/7 | [Journal identity](../experiments/benchmarks/hammer-reference-findings.md): replacement at the same offset, beyond-length offsets, discovery/indexing rotation and CRC corruption; three controls failed before repair, all four passed afterward | Append-only committed bytes and unique first-group identities; arbitrary movement/retry liveness unchecked |
| HIST-3/4/5 | [Range evidence](../experiments/benchmarks/catalog-range-evidence-findings.md): typed timestamp/error controls, borrowed ownership and late excluded-digest rejection; unchanged complete snapshot oracle and retention outcomes | Finite fixtures |
| PC boundedness; HIST-4/5 | [Ownership transitions](../experiments/benchmarks/catalog-transition-ownership-findings.md): cancellation/journal reopen, prebody admission, blocking permits and snapshot metadata; ten complete oracle-graded chains, two missing-row rejection and two Gone controls | No universal query-memory bound |
| DEL-1/2; HIST-1/2 | [Native lifecycle](../experiments/benchmarks/catalog-native-lifecycle-findings.md): producer/recovery byte equality, real collection/TLS, retry, restart, publication and reclaim; thirty full chains and twelve rejection controls. [Startup regressions](../../crates/fabric-server/tests/startup.rs) check journal ownership after missing/invalid TLS input | Finite lifecycle cases |
| PC custody/freshness; HIST-1 | [Progress and projection](../experiments/benchmarks/catalog-log-progress-findings.md): [skip-progress regressions](../../src/spindle/skip_progress_tests.rs) retain the stall and check restart/refusal/retry; [row ownership controls](../../crates/fabric-server/src/rows_ownership_tests.rs) check literal and frozen-extractor compatibility | Generated regressions supplement independent oracles; arbitrary-input liveness and service rate unchecked |

The [invariant audit](../experiments/formal/invariant-consolidation.md) adds
control-state size/reopen and uncertain-publication refusal regressions, Spool
sequence-exhaustion controls, and fail-closed laboratory completion receipts.
These preserve existing contract meanings; final execution outcomes are in that record.

## Delivery

| ID | Contract | Spec | Production implementation | Verification | Independent oracle | Negative control | Formal model | Fault test | Status | Unchecked |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| DEL-1 | Commit precedes ACK | PC custody; ADR-0005; ADR-0013 | `fabric_app::delivery::commit_group`; `Store` journal adapter | `ack_is_returned_only_after_a_successful_commit`; server `delivery` tests | delivery oracle `ACKED-DURABLE` | M-DEL-ACK; fault harness `--mutate drop-recovered` | [DeliveryOwnership](../../formal/delivery/README.md) `EarlyAck` counterexample (TLC v1.7.1); TLC trace validation of fault-run transcripts ([trace_check.py](../../formal/delivery/trace_check.py)) | `delivery_faults.py` server-kill, outage (rerun on this branch) | Tested | Physical power loss; fsync honored by the device |
| DEL-1a | A lone sender's group closes after 2 ms of quiet, not the 50 ms window; durability is unchanged (ADR-0013 amendment) | ADR-0013 | `CommitMode::quiet`, `Store::run` | `a_lone_submission_is_not_held_for_the_window` | — | `without_the_quiet_rule_a_lone_submission_waits_the_window` (quiet = window) | — | — | Tested | The delivery fault runs have not been rerun under the rule; the timing tests assume a quiet host |
| DEL-2 | A retry does not duplicate a logical Batch | ADR-0013 | `decide_delivery` `Duplicate`/`Stale` | truth table; differential test; `retry_conflict_gap_binding_and_rejections_follow_the_delivery_rule` | delivery oracle duplicate rule | oracle mutation control (duplicate record) | — | server-kill, outage | Tested | Retries across a generation change beyond the tested cases |
| DEL-3 | Same Strand and sequence with different bytes is rejected, never replaces | ADR-0013 | `decide_delivery` `Conflict` | truth table; differential; server delivery test | delivery oracle `EXACT-RETRY` | M-DEL-BYTES; oracle replaced-bytes control | — | — | Tested | SHA-256 collision (assumed infeasible) |
| DEL-4 | A sequence gap is detected, not committed | ADR-0013 | `decide_delivery` `Gap`; `next_sequence` | truth table incl. `u64::MAX`; differential | delivery oracle sequence rules | M-DEL-GAP | — | — | Tested | — |
| DEL-5 | Credential and Spindle binding cannot silently change, including within one group | ADR-0013 | `BindingState::permits`; `GroupPlan` | binding tests; differential over all consistent binding states | — | M-DEL-BIND, M-DEL-GROUP | — | — | Tested | Binding across checkpoint reload is covered only by the retention end-to-end test |

## Spool

| ID | Contract | Spec | Production implementation | Verification | Independent oracle | Negative control | Formal model | Fault test | Status | Unchecked |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| SPOOL-1 | Unacknowledged data stays retained | PC custody | `Spool::record_ack`, `next_unacked` | spool unit tests; Spindle tests; [native ENOSPC append](../../tests/spool_enospc.rs) preserves exact prior unACKed bytes, sequence and ACK cursor | delivery oracle `NODE-RETAINS-UNACKED` | oracle early-forget control; trace-check early-forget control; unmatched-path ENOSPC control | DeliveryOwnership `LostCopy`; TLC trace validation of fault-run transcripts | node-kill runs; actual scoped ENOSPC syscall ([continuation](../experiments/formal/readiness-continuation-results.md)) | Finite cases tested | Physical filesystem exhaustion and sustained outage; double fault while recording failure |
| SPOOL-2 | Reclaim removes only acknowledged closed files | PC custody | `Spool::record_ack` reclaim loop | `acknowledged_prefix_is_sent_in_order_then_reclaimed_by_whole_files` | — | M-SPOOL-RECLAIM | — | — | Tested | — |
| SPOOL-3 | Interrupted-append recovery does not clear a known I/O failure | ADR-0011 | `FrameLog::open` | `reported_sync_errors_quarantine_and_record_known_failure`; process-death stages; native ENOSPC quarantine and reopen refusal after fault removal | — | M-SPOOL-KNOWN; ENOSPC no-hit control | — | `kill_probe.py` (not rerun here); [continuation](../experiments/formal/readiness-continuation-results.md) ENOSPC fixture | Tested | Double fault while recording the failure (documented residual in ADR-0011) |
| SPOOL-4 | A log-heavy node keeps rotating and reclaiming its Spool between metric intervals, and every retained file still starts with host counter state (ADR-0025) | PC custody, boundedness | `Spool::rotation_due`, `Spindle::collect` | `a_log_heavy_node_rotates_its_spool_between_metric_intervals` | — | the same test fails with the rotation-due sampling removed | — | spool-full exit seen in spindle run 01 before the fix | Tested | Rotation when host metrics cannot be sampled at all |
| METER-1 | The Spindle reports exactly what it committed and delivered, and its output cap holds delivery to the configured rate without refusing a Batch (ADR-0025) | PC boundedness | `spindle::meter::Meter`, `Spindle::deliver` | unit tests for the bucket and counters; `the_spindle_reports_what_it_committed_in_its_next_metric_cycle`; `the_output_cap_holds_delivery_to_its_rate` (real server); spindle run 01 (4 MiB/s cap) | — | — | — | — | Tested | Counters across a restart are reset, not continued (by design) |

## Control

The [Btrfs identity regression](../experiments/formal/btrfs-cursor-protocol.md)
supplements Spool/source-cursor verification: full filesystem/subvolume matching,
raw-device legacy migration, replacement/truncation/prefix controls, failed probe,
quiet-file commit and full-Spool refusal/retry passed their focused native tests
on `889dfac`. The failed Fedora 51-to-32 reboot is retained as
`CX-BTRFS-REBOOT-LOG-REPLAY`; corrected exact-package lifecycle acceptance remains
pending. This does not establish portable identity for other filesystems.

| ID | Contract | Spec | Production implementation | Verification | Independent oracle | Negative control | Formal model | Fault test | Status | Unchecked |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CTRL-1 | An invalid desired configuration does not become active | PC control; ADR-0014 | Spindle `effective` + `Config::validate`; server `check_desired` | `admin_api_configures_pauses_and_revokes_a_polling_node` | — | M-CTRL-INVALID | — | — | Tested | Spindle-side validation (`Config::validate`) is not in the core; server shape checks are (`fabric_core::control::check_desired`) |
| CTRL-2 | A revoked identity cannot continue sending, and revocation is terminal | PC control; ADR-0014 | `fabric_core::control` (`authorizes`, `set_status`); `Control::change`, `authenticate` | same end-to-end test; kernel truth tests; differential against the base | — | M-CTRL-REVOKE, M-CTRL-TERMINAL | — | — | Tested | Revocation latency under load |
| CTRL-3 | The last valid local configuration survives disconnection and restart | PC product boundary | `applied-config.json`, `read_applied` | same end-to-end test (offline restart) | — | none registered | — | — | Tested | No mutant yet |

## History and query

| ID | Contract | Spec | Production implementation | Verification | Independent oracle | Negative control | Formal model | Fault test | Status | Unchecked |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| HIST-1 | Journal and Segment representations answer equivalently | PC completeness; RH | `History::run` over segments and journal tail | `journal_and_segment_representations_answer_identically` (journal only versus Segment only); oracle-graded mixed answers; [streaming output byte-equivalence pilot](../experiments/benchmarks/streaming-output-local-run-01.md) | query oracle | M-HIST-SEGMENT | — | — | Tested | Histograms and summaries (no rows by contract); non-string attributes |
| HIST-2 | Sealing does not change a query result | RH | sealer + `History` | the same test, before and after sealing; crash-state test; [reclaim scheduling and real checkpoint/retry fixtures](../experiments/benchmarks/journal-reclaim-local-run-01.md) | query oracle | M-HIST-SEGMENT | — | sealing crash states | Tested | Sealing concurrent with retention |
| HIST-3 | A missing or corrupt Segment never yields a complete answer | PC completeness; RH | `History::run` `unavailable` | `a_corrupt_segment_makes_the_answer_incomplete`; missing raw table and footer/schema/count cuts in `completion_storage` ([completion evidence](../experiments/benchmarks/coupled-completion-run-01.md)) | unchanged whole-record query oracle; scoped producer-derived projection-availability companion | M-HIST-CORRUPT; oracle completeness mutants; companion row/envelope mutations | — | truncated segment; CX-RAW-TABLE-COMPLETE | Finite cases tested | Silent bit rot inside a readable Parquet page (whole-file SHA-256 not checked per query); raw-loss companion covers matching nonempty queries only |
| HIST-4 | Pagination stays bound to one snapshot | RH | page token `(oldest, newest, key)` | `sealed_history_answers_exactly_and_pages_are_stable` | query oracle pagination rules | oracle pagination mutants | — | — | Tested | — |
| HIST-5 | Retention produces correct Gone and incomplete semantics | RH | `fabric_core::retention`, `fabric_app::retention::apply_retention`; `fabric_core::query::page_snapshot_retained` | `retention_removes_old_segments_and_stream_state_survives_it`; [continuation](../experiments/formal/readiness-continuation-results.md) positive B/B−1 boundaries, long-lived readers and interrupted-deletion restart preserve the declared suffix | query oracle | M-HIST-GONE, M-RET-AGE; missing-row control | — | interrupted deletion/restart | Finite cases tested | Retention under the 20 GiB byte limit at scale; arbitrary concurrent schedules |
| HIST-6 | Counter resets produce reset markers, not rates | RH; PC fidelity | `fabric_core::query::counter_step`, refined by `counter_step_numbers` and used by `rates` | history rate query graded by the oracle; bit-exact differential against the base rule; `rate_counterexamples_survive_delivery_sealing_and_restart` (identity, integer/mixed precision, 2,048 series, HTTP, both plans, sealing/restart) | query/rate oracle | M-HIST-RESET; oracle rate mutants | — | — | Tested | Series grouping and ordering stay in the server |
| HIST-7 | The walk plan returns the scan plan's answer, page for page (ADR-0024 part 1) | RH; QS-2 | `History` with `Plan::Walk`: `sources_walk`, `walk_order`, `tail::WalkState`, `tail::TailReader` | `walk_and_scan_plans_answer_identically` (190 seeded queries over sealed and unsealed history, three Spindles at identical instants, a full drain); `a_walking_server_keeps_its_tail_index_exact_while_sealing`; walk checks in the corrupt-Segment and sealing-crash tests; query walk run 01 (400 random queries) | query oracle on the walk's own answers | injected `>=` in the stop rule of the logs and of the metrics walk: both fail the equivalence test | QS-2's `threshold_walk` property (the rule, not the server's code) | sealing crash states; truncated Segment | Tested | Concurrent queries against the index mutex under load; a model-based test of the server walk against `spec` |
| HIST-8 | A Segment's text filter never hides a matching row from the walk, and a missing or corrupt filter falls back to the exact scan (ADR-0024 part 2) | PC optional indexes; RH | `text_filter::GroupFilter`, `segment::write_text_filter`, `segment::read_text_filter`, `walk_order` | unit tests `a_group_never_rejects_a_substring_of_its_bodies`, `the_encoding_round_trips_and_refuses_other_forms`; `text_filters_skip_only_groups_without_the_needle_and_fall_back_when_corrupt`; HIST-7's tests over filtered Segments; text filter run 01 (400 random queries) | query oracle through HIST-7 | a lying filter with a forged manifest digest: the equivalence check must see the dropped rows | — | zeroed filter bytes | Tested | A faulty sealer that writes a wrong filter and its digest (the filter's trust assumption, as for any Segment file); filters at 8,192-line groups |
| HIST-9 | The walk over tail blocks returns the scan plan's answer, and a block the codec refuses leaves its entries on the journal path (ADR-0024 part 3) | RH; ADR-0023 | `tail::Pending`, `tail::visit_block`, `tail::log_row`, `tail::metric_row`; `TailBlockSource` in `query.rs` | `a_block_tail_answers_as_the_scan_does` (blocks of 60 records, sealed and unsealed history, three Spindles at identical instants, a full drain); unit test `a_block_the_codec_refuses_is_not_made`; block tail run 01 (400 random queries) | query oracle through HIST-7 | an entry-boundary defect in `visit_block` (records attributed to the previous entry) fails the equivalence test | the FOB1 canonical-encoding properties (OBS-1/2) | — | Tested | Block build cost after a restart at the 4 GiB journal ceiling; memory of blocks under a long sealer outage |
| TRACE-1 | A trace export is acknowledged only after it is in the Spool, and its spans are retained and answered exactly (ADR-0025) | PC fidelity and custody; RH | `spindle::otlp`, `Spindle::commit_traces`, envelope field 9, `rows::SpanRow`, `spans.parquet`, the `spans` query in both plans | `trace_exports_are_committed_to_the_spool_before_they_are_acknowledged` (binary); `exported_spans_reach_the_server_and_answer_the_spans_query` (end to end, custody byte equality); history tests with three spans per Batch, oracle-graded span queries and plan equivalence | query oracle (spans, field numbers from the vendored proto) | oracle span mutants (dropped, reordered, wrong parent, extra key); a wrong block filter for trace lookups fails the block-tail test | — | out-of-range span kind (block refused, entries read from the journal) | Tested | gRPC export; exporters that send gzip bodies; trace exports under a full Spool for longer than an exporter's retry budget |

## Architecture

| ID | Contract | Spec | Verification | Negative control | Status | Unchecked |
| --- | --- | --- | --- | --- | --- | --- |
| ARCH-1 | Dependencies point inward | ADR-0015 | `cargo xtask check-layers` | fixture defects in `xtask/tests/gates.rs` (every forbidden edge kind); repository-level injected edge | Tested | Module structure inside composition roots |
| ARCH-2 | The semantic core is pure | ADR-0016 | `cargo xtask check-core-purity`; `no_std`; Clippy restrictions | fixture defects in `xtask/tests/gates.rs`; repository-level injected `ureq` and `extern crate std` | Tested | Per-function determinism beyond what the structure forbids |

## Checks added in the verification-tooling milestone

These checks complement the Verification column above ([ADR-0021](../decisions/ADR-0021-add-property-model-fuzz-and-simulation-checks.md)). Check IDs and profile membership are in [xtask/checks.json](../../xtask/checks.json); mutant IDs are in [xtask/mutants.json](../../xtask/mutants.json). Registration alone is not an execution result.

| Contract | Property test (`kernel-properties`) | Kani proof (`kani-core`) | Fuzz (`fuzz-corpus`, `fuzz-smoke`) | Network simulation (`network-simulation`) | Negative controls |
| --- | --- | --- | --- | --- | --- |
| DEL-1 | — | — | — | acknowledged Batches are retained exactly once after lost answers | M-SIM-DUPLICATE |
| DEL-2 | commit groups decide like one Batch at a time; contiguous immutable history | — | — | a retry after a lost answer is acknowledged | M-PROP-GROUP, M-SIM-DUPLICATE |
| DEL-3 | the ADR-0013 table | the ADR-0013 table over every `u64` sequence | — | — | M-KANI-DUP |
| DEL-4 | the ADR-0013 table | no successor of `u64::MAX` | — | sequences 1..n once each | M-KANI-SEQ |
| DEL-5 | bindings within a commit group | — | an invalid Batch is refused before identification | — | M-PROP-GROUP, M-FUZZ-IDENTIFY |
| SPOOL-1 | cursor identity, counter starts, bounded gap texts | the same, over full ranges | — | — | M-PROP-TEXT |
| SPOOL-3 | — | — | frame-log recovery reaches a fixed point | — | — |
| CTRL-1 | desired-configuration and name limits | — | — | — | — |
| CTRL-2 | revocation is terminal over any request sequence | revocation is terminal | — | — | — |
| HIST-4 | pages cover every row once, in order | — | query requests and page tokens never panic | — | M-PROP-PAGE |
| HIST-5 | the shortest sufficient retention prefix | the same, up to three Segments over full ranges | — | — | M-PROP-RET-BYTES, M-KANI-RET-SATURATE |
| HIST-6 | rates only from uninterrupted series | rates finite and non-negative | — | — | CX-RATE-NON-FINITE regression |

## Sealer (defaults adopted and verified)

Claims of the [sealer design](../architecture/sealer.md)
([ADR-0022](../decisions/ADR-0022-build-segments-by-external-merge-sort.md)).
The [combined campaign](../experiments/formal/encoded-page-memory-run-01.md)
passed eight registered cells with three reference/candidate pairs each;
measurements belong to its frozen experimental sources. The
[native R2 trial](../experiments/benchmarks/soak-run-02.md) passed all ten gates.
Both full-trial and default-verification freezes were archived, verified and removed.
The [default run](../experiments/formal/bounded-writer-default-run-01.md)
records 27 unflagged bounded tests, 13 kill cuts plus no-hit and loaded-content
binary equivalence with a rejected byte-flip control. These checks support the
adopted defaults without claiming a second counting campaign. Historical
screens, the entropy counterexample and the original failed soak remain preserved.
The [continuation](../experiments/formal/readiness-continuation-results.md)
records syscall faults, mutants, the preceding fast/dependency checks and receipts.
All 17 final unflagged fast checks and three manual documentation checks passed.
CI/merge and deployment qualification are not claimed. Invariants and planned
controls below remain unchanged.

| ID | Contract | Spec | Planned verification | Planned negative control | Finite evidence | Acceptance / remaining limits |
| --- | --- | --- | --- | --- | --- | --- |
| SEAL-1 | The Segment holds exactly the journal file's rows and records | S-1; HIST-1, HIST-2 | differential test against the current `segment::build`; query oracle on mixed answers | — | Small differential fixture; continuation eight-cell/three-pair opt-in campaign passed exact rows, manifests and applicable byte equality; mixed-signal kill-cut queries independently graded | Default bounded/recovery tests and loaded-content equivalence passed; arbitrary real application bodies and files above 256 MiB unchecked; historical physical-count failure retained |
| SEAL-2 | Merged rows follow the contract's order key, so row-group time ranges do not overlap | S-2; RH query order | merge property test over any rows and run size; read amplification equal to the current build's | M-SEAL-MERGE-ORDER | Continuation: 64 seeds × five byte limits, thirteen fan-in populations, stable ties and altered-key/payload/order controls; named mutant caught; opt-in campaign passed strict pruning equality | Finite generators do not establish arbitrary rows/run sizes; campaign pruning measurements retain their frozen revision |
| SEAL-3 | Peak heap is bounded and does not grow with the file | S-3 | counting-allocator test on four workloads; scale test from 64 to 256 MiB | M-SEAL-RETAIN-RUNS | Combined aligned-input/disk-PageStore eight-cell/three-pair campaign passed the 80 MiB ceiling and 10% scaling gates; high-entropy correction preserved exact output; named retain-runs mutant caught by steady128 ceiling. The encoded-page record retains current measurements and preceding aligned-only results/counterexample | Native R2 also passed with a live commit thread; arbitrary shapes and a whole-server bound remain unproven |
| SEAL-4 | No run file or build directory outlives its build | S-4 | fault tests: injected read, write and out-of-space errors | M-SEAL-SPILL-LEFT | Continuation seventeen actual scoped syscall faults recovered exact manifests; ordinary-error cleanup checked before retry; named mutant caught; success cleanup in repeated opt-in campaign; seven native page-store write/read/partial-I/O/kill/cleanup faults passed exact retry and cleanup checks | Finite named faults; physical disk exhaustion, double faults and arbitrary crashes during cleanup unchecked |
| SEAL-5 | The journal file is deleted only after the Segment's rename, and a crash at any stage loses no record | S-5; HIST-2 | the crash-state test extended to the new stages | — | Constructed crash states and checkpoint retry; continuation thirteen named SIGKILL cuts plus no-hit control preserve exact custody and all five query shapes in Scan/Walk before and after restart, graded by unchanged query oracle; missing-row control rejected; all thirteen cuts plus no-hit also passed unflagged after default adoption | Representative spill/merge/table/filter/manifest/directory/publication cuts; arbitrary instruction boundaries and physical power loss unchecked |
| SEAL-6 | The same input gives the same files | S-6 | two builds compared by manifest hash | — | Small differential fixture; continuation opt-in campaign passed repeatability across three pairs in all eight cells | Measured workloads and frozen constants only; default verification establishes loaded-content equivalence, not another repeated counting campaign |

## Qualification

These are operating-profile claims; none has recorded target-profile qualification. A measurement names the revision whose frozen binaries it ran. The history, outage, stress and soak runs used byte-identical builds of the Rust sources at `63bbeac`, the SHA-256 values in their records. See the [capability ledger](../QUALIFICATION.md#capability-ledger).

| ID | Claim | Gate | Latest measurement | Recorded evidence status |
| --- | --- | --- | --- | --- |
| QUAL-MEM | Memory bound | native RSS ≤ 64 MiB; central RSS ≤ 2 GiB | native run 02 (`c51d3a8`); fleet run 01 (`4921e5e`); outage run 01 (node ≤ 5.2 MiB); stress run 01 and soak run 01 (server ≤ 648 MiB). The soak's own RSS-growth gate failed ([soak run 01](../experiments/benchmarks/soak-run-01.md)) | Measured on `2b5c939` (not target profile) |
| QUAL-DISK | Disk bound | 5 GiB live data per invocation; Spool and retention limits | fleet run 01 live bytes; history run 01 (≤ 480 MB), stress run 01 (≤ 398 MB) and soak run 01 (≤ 968 MB) peak live bytes under the 5 GiB runner limit; outage run 01 peak Spool 3.7 MB of 256 MiB. Retention limits are not reached in any run | Measured on `2b5c939` (not target profile) |
| QUAL-ACK | Delivery latency | ACK p99 ≤ 1 s | delivery run 01 (`71fef99`); fleet run 01; soak run 01 (`2b5c939`, four-CPU host): ACK p99 ≤ 63.6 ms in every 600 s window over 91 min | Measured on `2b5c939` (not target profile) |
| QUAL-FRESH | Freshness | observation to query p99 ≤ 5 s at 1,000 identities | history run 01 under revision 2, four-CPU host (`63bbeac`): p99 ≤ 1.78 s | Measured on `63bbeac` (not target profile) |
| QUAL-QLAT | Query latency | p99 ≤ 2 s over 1,000,000 records | history run 01 under revision 2, four-CPU host (`63bbeac`): p99 ≤ 481 ms Segments, ≤ 1,515 ms journal-only | Measured on `63bbeac` (not target profile) |
| QUAL-DRAIN | Outage drain | 30 min buffered, drained ≤ 10 min | outage run 01 (`bb8d06d`, four-CPU host): drain ≤ 99 s, oracle exact | Measured on `bb8d06d` (not target profile) |
| QUAL-INSTALL | Installation behavior | running-installation acceptance | [installation acceptance run 01](../experiments/formal/installation-acceptance-run-01.md) (`bbd2dd5`, container on a legacy cgroup hierarchy): 16 of 17 checks passed, three mutations rejected, `MemoryHigh` not run | Inconclusive |
| QUAL-H | Harness correctness | runner limits, rate oracle | test suites | Tested |

## Server self-observation (ADR-0026)

The additional CLI self-observation capability is tracked separately in
[its protocol and findings](../experiments/formal/server-self-observation-findings.md).
`operational_log` unit tests check bounded rotation, unsafe files and locking;
`local_logs_tests` checks source pins and exact committed bodies across restart;
`self_identity` checks credential publication and terminal revocation. The native
smoke verifies real diagnostic queries and child ownership with dropped, changed
and duplicate-body controls. These finite checks do not advance `QUAL-INSTALL`
or change the meaning of any registered delivery/query/rate oracle. The
[continuation](../experiments/formal/readiness-continuation-results.md) adds three
actual-supervisor shutdown controls: orderly child exit succeeds; nonzero exit
and a forced deadline kill fail the server result, with every child reaped. Both
named shutdown mutants were caught after passing baselines.

## Observation record (accepted for the Walk tail, ADR-0023)

Claims of the [fabric-observation](../../crates/fabric-observation/src/lib.rs) codec, adopted by [ADR-0023](../decisions/ADR-0023-define-an-observation-record-with-a-canonical-encoding.md) for the production Walk plan's in-memory journal-tail blocks (HIST-9). Wire, journal and Segment formats retain Batch bytes. OBS-1/2 are not in the registered contract list of `xtask/checks.json`; workspace tests and fuzz corpus replay nevertheless exercise the codec, while its Kani harnesses remain outside the registered proof check.

| ID | Contract | Spec | Production implementation | Verification | Independent oracle | Negative control | Formal model | Fault test | Status | Unchecked |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| OBS-1 | Every valid block decodes to itself and has exactly one byte string | [ADR-0023](../decisions/ADR-0023-define-an-observation-record-with-a-canonical-encoding.md) | `fabric_observation::encode`, `decode` over the [tower](../architecture/observation.md) | `round_trip_all_three_signals`; property `round_trip_is_identity` (1,500 cases); per-level properties in `observation_levels.rs` (bits, crc32, varint, zigzag, delta, dictionary; 3,000 cases each) | — | `rejects_non_canonical_records_before_encoding` (unsorted and duplicate keys, NaN, severity 25) | Kani harnesses for zigzag and delta (`cfg(kani)`, not in the registered check) | — | Tested | Blocks above 40 records in the property generator |
| OBS-2 | Every accepted byte string is the canonical encoding of its decoding, and no input panics the decoder | ADR-0023 | `decode`'s canonicality checks | `single_bit_flips_are_rejected_or_canonical`; properties `mutations_are_rejected_or_canonical`, `arbitrary_bytes_never_panic`; fuzz target `observation_block` (corpus replay in `fuzz-corpus`) | — | `rejects_overlong_varints`, `rejects_dictionaries_out_of_first_use_order_or_unused` (including [CX-FOB1-DUPLICATE-DICTIONARY](counterexamples.json), found by the mutation property), `rejects_crc_mismatch_truncation_and_trailing_bytes`; fuzz regression `duplicate-dictionary.fob` | — | — | Tested | Coverage-guided fuzzing beyond the committed corpus (needs the nightly `fuzz-smoke`) |

## Query specification

The executable definition of a query answer ([query::spec](../../crates/fabric-core/src/query/spec.rs); [query-semantics.md](query-semantics.md)) and its theorems. The read-path mechanisms are refinements of it; the stock server is a regression reference, not an oracle.

| ID | Contract | Spec | Production implementation | Verification | Independent oracle | Negative control | Formal model | Fault test | Status | Unchecked |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| QS-1 | Pages partition the admitted rows in key order (T2) and the definition is invariant to input order, snapshot-hidden rows, and splits of the window (T3, T6); limits are prefixes (T4); filters restrict (T5) | [query-semantics.md](query-semantics.md) | `spec::logs`, `spec::drain` | properties `pages_partition_the_answer`, `the_snapshot_hides_other_groups`, `windows_split`, `a_smaller_limit_is_a_prefix`, `a_node_filter_restricts` (3,000 cases) | none needed: relations of the definition with itself | — | Kani attempted at three and four rows, did not finish in 40 min; not in the registered file | — | Tested | Metric points and rates as a separate instance; an unbounded proof |
| QS-2 | The threshold walk over sources with sound bounds equals the definition (T7); the budget boundary drains to the definition and every budgeted page is an exact prefix (T8) | query-semantics.md | `spec::threshold_walk`, `spec::budgeted_walk`; the server's walk and budget prototypes are refinements | properties `the_walk_equals_the_definition`, `the_budget_drains_to_the_definition` | — | `unsound_bounds_can_break_the_walk` (a lying bound) | Kani attempted, did not finish; not in the registered file | — | Tested | The server's implementations are checked by differential against stock, not yet by a model-based test against `spec` |
