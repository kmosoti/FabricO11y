# Verification matrix

A living map from each product claim to what checks it on the **current head**. Status uses the [evidence states](../QUALIFICATION.md#evidence-states); "Tested" means a check in the fast [check registry](../../xtask/checks.json) exercises the claim and passed on this branch. Historical run records belong to the revisions they name and are listed only as measurement evidence. Semantic mutant IDs refer to [xtask/mutants.json](../../xtask/mutants.json); their latest results are in the [milestone record](../milestones/architecture-foundation.md#semantic-mutants).

Specification sources: **PC** = [product contract](../PRODUCT-CONTRACT.md), **ADR-n** = [decisions](../decisions/README.md), **RH** = [retained history](../architecture/retained-history.md), **DO**/**QO** = [delivery](../../tools/qualification/DELIVERY_ORACLE.md)/[query](../../tools/qualification/QUERY_ORACLE.md) oracle specifications.

## Delivery

| ID | Contract | Spec | Production implementation | Verification | Independent oracle | Negative control | Formal model | Fault test | Status | Unchecked |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| DEL-1 | Commit precedes ACK | PC custody; ADR-0005; ADR-0013 | `fabric_app::delivery::commit_group`; `Store` journal adapter | `ack_is_returned_only_after_a_successful_commit`; server `delivery` tests | delivery oracle `ACKED-DURABLE` | M-DEL-ACK; fault harness `--mutate drop-recovered` | [DeliveryOwnership](../../formal/delivery/README.md) `EarlyAck` counterexample (TLC v1.7.1); TLC trace validation of fault-run transcripts ([trace_check.py](../../formal/delivery/trace_check.py)) | `delivery_faults.py` server-kill, outage (rerun on this branch) | Tested | Physical power loss; fsync honored by the device; no trace-to-model mapping |
| DEL-2 | A retry does not duplicate a logical Batch | ADR-0013 | `decide_delivery` `Duplicate`/`Stale` | truth table; differential test; `retry_conflict_gap_binding_and_rejections_follow_the_delivery_rule` | delivery oracle duplicate rule | oracle mutation control (duplicate record) | — | server-kill, outage | Tested | Retries across a generation change beyond the tested cases |
| DEL-3 | Same Strand and sequence with different bytes is rejected, never replaces | ADR-0013 | `decide_delivery` `Conflict` | truth table; differential; server delivery test | delivery oracle `EXACT-RETRY` | M-DEL-BYTES; oracle replaced-bytes control | — | — | Tested | SHA-256 collision (assumed infeasible) |
| DEL-4 | A sequence gap is detected, not committed | ADR-0013 | `decide_delivery` `Gap`; `next_sequence` | truth table incl. `u64::MAX`; differential | delivery oracle sequence rules | M-DEL-GAP | — | — | Tested | — |
| DEL-5 | Credential and Spindle binding cannot silently change, including within one group | ADR-0013 | `BindingState::permits`; `GroupPlan` | binding tests; differential over all consistent binding states | — | M-DEL-BIND, M-DEL-GROUP | — | — | Tested | Binding across checkpoint reload is covered only by the retention end-to-end test |

## Spool

| ID | Contract | Spec | Production implementation | Verification | Independent oracle | Negative control | Formal model | Fault test | Status | Unchecked |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| SPOOL-1 | Unacknowledged data stays retained | PC custody | `Spool::record_ack`, `next_unacked` | spool unit tests; Spindle tests | delivery oracle `NODE-RETAINS-UNACKED` | oracle early-forget control; trace-check early-forget control | DeliveryOwnership `LostCopy`; TLC trace validation of fault-run transcripts | node-kill runs | Tested | Disk full on the Spindle during an outage (registered, not run) |
| SPOOL-2 | Reclaim removes only acknowledged closed files | PC custody | `Spool::record_ack` reclaim loop | `acknowledged_prefix_is_sent_in_order_then_reclaimed_by_whole_files` | — | M-SPOOL-RECLAIM | — | — | Tested | — |
| SPOOL-3 | Interrupted-append recovery does not clear a known I/O failure | ADR-0011 | `FrameLog::open` | `reported_sync_errors_quarantine_and_record_known_failure`; process-death stages | — | M-SPOOL-KNOWN | — | `kill_probe.py` (not rerun here) | Tested | Double fault while recording the failure (documented residual in ADR-0011) |

## Control

| ID | Contract | Spec | Production implementation | Verification | Independent oracle | Negative control | Formal model | Fault test | Status | Unchecked |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CTRL-1 | An invalid desired configuration does not become active | PC control; ADR-0014 | Spindle `effective` + `Config::validate`; server `check_desired` | `admin_api_configures_pauses_and_revokes_a_polling_node` | — | M-CTRL-INVALID | — | — | Tested | Spindle-side validation (`Config::validate`) is not in the core; server shape checks are (`fabric_core::control::check_desired`) |
| CTRL-2 | A revoked identity cannot continue sending, and revocation is terminal | PC control; ADR-0014 | `fabric_core::control` (`authorizes`, `set_status`); `Control::change`, `authenticate` | same end-to-end test; kernel truth tests; differential against the base | — | M-CTRL-REVOKE, M-CTRL-TERMINAL | — | — | Tested | Revocation latency under load |
| CTRL-3 | The last valid local configuration survives disconnection and restart | PC product boundary | `applied-config.json`, `read_applied` | same end-to-end test (offline restart) | — | none registered | — | — | Tested | No mutant yet |

## History and query

| ID | Contract | Spec | Production implementation | Verification | Independent oracle | Negative control | Formal model | Fault test | Status | Unchecked |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| HIST-1 | Journal and Segment representations answer equivalently | PC completeness; RH | `History::run` over segments and journal tail | `journal_and_segment_representations_answer_identically` (journal only versus Segment only); oracle-graded mixed answers | query oracle | M-HIST-SEGMENT | — | — | Tested | Histograms and summaries (no rows by contract); non-string attributes |
| HIST-2 | Sealing does not change a query result | RH | sealer + `History` | the same test, before and after sealing; crash-state test | query oracle | M-HIST-SEGMENT | — | sealing crash states | Tested | Sealing concurrent with retention |
| HIST-3 | A missing or corrupt Segment never yields a complete answer | PC completeness; RH | `History::run` `unavailable` | `a_corrupt_segment_makes_the_answer_incomplete` | query oracle `complete` rule | M-HIST-CORRUPT; oracle completeness mutants | — | truncated segment | Tested | Silent bit rot inside a readable Parquet page (whole-file SHA-256 not checked per query) |
| HIST-4 | Pagination stays bound to one snapshot | RH | page token `(oldest, newest, key)` | `sealed_history_answers_exactly_and_pages_are_stable` | query oracle pagination rules | oracle pagination mutants | — | — | Tested | — |
| HIST-5 | Retention produces correct Gone and incomplete semantics | RH | `fabric_core::retention`, `fabric_app::retention::apply_retention`; `fabric_core::query::page_snapshot_retained` | `retention_removes_old_segments_and_stream_state_survives_it` | query oracle | M-HIST-GONE, M-RET-AGE | — | — | Tested | Retention under the 20 GiB byte limit at scale |
| HIST-6 | Counter resets produce reset markers, not rates | RH; PC fidelity | `fabric_core::query::counter_step`, used by `rates` | history rate query graded by the oracle; bit-exact differential against the base rule | query/rate oracle | M-HIST-RESET; oracle rate mutants | — | — | Tested | Series grouping and ordering stay in the server |

## Architecture

| ID | Contract | Spec | Verification | Negative control | Status | Unchecked |
| --- | --- | --- | --- | --- | --- | --- |
| ARCH-1 | Dependencies point inward | ADR-0015 | `cargo xtask check-layers` | fixture defects in `xtask/tests/gates.rs` (every forbidden edge kind); repository-level injected edge | Tested | Module structure inside composition roots |
| ARCH-2 | The semantic core is pure | ADR-0016 | `cargo xtask check-core-purity`; `no_std`; Clippy restrictions | fixture defects in `xtask/tests/gates.rs`; repository-level injected `ureq` and `extern crate std` | Tested | Per-function determinism beyond what the structure forbids |

## Qualification

These are operating-profile claims; none is qualified on the current head. A measurement names the revision whose frozen binaries it ran. The history, outage, stress and soak runs used byte-identical builds of the Rust sources at `63bbeac`, the SHA-256 values in their records. See the [capability ledger](../QUALIFICATION.md#capability-ledger).

| ID | Claim | Gate | Latest measurement | Status on current head |
| --- | --- | --- | --- | --- |
| QUAL-MEM | Memory bound | native RSS ≤ 64 MiB; central RSS ≤ 2 GiB | native run 02 (`c51d3a8`); fleet run 01 (`4921e5e`); outage run 01 (node ≤ 5.2 MiB); stress run 01 and soak run 01 (server ≤ 648 MiB). The soak's own RSS-growth gate failed ([soak run 01](../experiments/benchmarks/soak-run-01.md)) | Measured on `2b5c939` (not target profile) |
| QUAL-DISK | Disk bound | 5 GiB live data per invocation; Spool and retention limits | fleet run 01 live bytes | Not run |
| QUAL-ACK | Delivery latency | ACK p99 ≤ 1 s | delivery run 01 (`71fef99`); fleet run 01; soak run 01 (`2b5c939`, four-CPU host): ACK p99 ≤ 63.6 ms in every 600 s window over 91 min | Measured on `2b5c939` (not target profile) |
| QUAL-FRESH | Freshness | observation to query p99 ≤ 5 s at 1,000 identities | history run 01 under revision 2, four-CPU host (`63bbeac`): p99 ≤ 1.78 s | Measured on `63bbeac` (not target profile) |
| QUAL-QLAT | Query latency | p99 ≤ 2 s over 1,000,000 records | history run 01 under revision 2, four-CPU host (`63bbeac`): p99 ≤ 481 ms Segments, ≤ 1,515 ms journal-only | Measured on `63bbeac` (not target profile) |
| QUAL-DRAIN | Outage drain | 30 min buffered, drained ≤ 10 min | outage run 01 (`bb8d06d`, four-CPU host): drain ≤ 99 s, oracle exact | Measured on `bb8d06d` (not target profile) |
| QUAL-INSTALL | Installation behavior | running-installation acceptance | [installation acceptance run 01](../experiments/formal/installation-acceptance-run-01.md) (`bbd2dd5`, container on a legacy cgroup hierarchy): 16 of 17 checks passed, three mutations rejected, `MemoryHigh` not run | Inconclusive |
| QUAL-H | Harness correctness | runner limits, rate oracle | test suites | Tested |
