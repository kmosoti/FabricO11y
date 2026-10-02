# Verification matrix

A living map from each product claim to what checks it on the **current head**. Status uses the [evidence states](../QUALIFICATION.md#evidence-states); "Tested" means a check in the fast [check registry](../../xtask/checks.json) exercises the claim and passed on this branch. Historical run records belong to the revisions they name and are listed only as measurement evidence. Semantic mutant IDs refer to [xtask/mutants.json](../../xtask/mutants.json); their latest results are in the [milestone record](../milestones/architecture-foundation.md#semantic-mutants).

Specification sources: **PC** = [product contract](../PRODUCT-CONTRACT.md), **ADR-n** = [decisions](../decisions/README.md), **RH** = [retained history](../architecture/retained-history.md), **DO**/**QO** = [delivery](../../tools/qualification/DELIVERY_ORACLE.md)/[query](../../tools/qualification/QUERY_ORACLE.md) oracle specifications.

## Delivery

| ID | Contract | Spec | Production implementation | Verification | Independent oracle | Negative control | Formal model | Fault test | Status | Unchecked |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| DEL-1 | Commit precedes ACK | PC custody; ADR-0005; ADR-0013 | `fabric_app::delivery::commit_group`; `Store` journal adapter | `ack_is_returned_only_after_a_successful_commit`; server `delivery` tests | delivery oracle `ACKED-DURABLE` | M-DEL-ACK; fault harness `--mutate drop-recovered` | [DeliveryOwnership](../../formal/delivery/README.md) `EarlyAck` counterexample (TLC v1.7.1); TLC trace validation of fault-run transcripts ([trace_check.py](../../formal/delivery/trace_check.py)) | `delivery_faults.py` server-kill, outage (rerun on this branch) | Tested | Physical power loss; fsync honored by the device |
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

## Checks added in the verification-tooling milestone

These run on the current head in addition to the Verification column above ([ADR-0021](../decisions/ADR-0021-add-property-model-fuzz-and-simulation-checks.md)). Check IDs refer to [xtask/checks.json](../../xtask/checks.json); mutant IDs to [xtask/mutants.json](../../xtask/mutants.json).

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

## Sealer (design accepted, not implemented)

Claims of the [sealer design](../architecture/sealer.md) ([ADR-0022](../decisions/ADR-0022-build-segments-by-external-merge-sort.md)). No check exists yet, so every status is **Not run**; the [milestone](../milestones/bounded-sealer.md) names the check each claim will get, and its mutants are registered in a policy commit before the implementation.

| ID | Contract | Spec | Planned verification | Planned negative control | Status | Unchecked |
| --- | --- | --- | --- | --- | --- | --- |
| SEAL-1 | The Segment holds exactly the journal file's rows and records | S-1; HIST-1, HIST-2 | differential test against the current `segment::build`; query oracle on mixed answers | — | Not run | Real log bodies; files above 256 MiB |
| SEAL-2 | Merged rows follow the contract's order key, so row-group time ranges do not overlap | S-2; RH query order | merge property test over any rows and run size; read amplification equal to the current build's | M-SEAL-MERGE-ORDER | Not run | — |
| SEAL-3 | Peak heap is bounded and does not grow with the file | S-3 | counting-allocator test on four workloads; scale test from 64 to 256 MiB | M-SEAL-RETAIN-RUNS | Not run | Interaction with a live commit thread |
| SEAL-4 | No run file or build directory outlives its build | S-4 | fault tests: injected read, write and out-of-space errors | M-SEAL-SPILL-LEFT | Not run | A crash during cleanup itself |
| SEAL-5 | The journal file is deleted only after the Segment's rename, and a crash at any stage loses no record | S-5; HIST-2 | the crash-state test extended to the new stages | — | Not run | Physical power loss |
| SEAL-6 | The same input gives the same files | S-6 | two builds compared by manifest hash | — | Not run | Different constants |

## Qualification

These are operating-profile claims; none is qualified on the current head. A measurement names the revision whose frozen binaries it ran. The history, outage, stress and soak runs used byte-identical builds of the Rust sources at `63bbeac`, the SHA-256 values in their records. See the [capability ledger](../QUALIFICATION.md#capability-ledger).

| ID | Claim | Gate | Latest measurement | Status on current head |
| --- | --- | --- | --- | --- |
| QUAL-MEM | Memory bound | native RSS ≤ 64 MiB; central RSS ≤ 2 GiB | native run 02 (`c51d3a8`); fleet run 01 (`4921e5e`); outage run 01 (node ≤ 5.2 MiB); stress run 01 and soak run 01 (server ≤ 648 MiB). The soak's own RSS-growth gate failed ([soak run 01](../experiments/benchmarks/soak-run-01.md)) | Measured on `2b5c939` (not target profile) |
| QUAL-DISK | Disk bound | 5 GiB live data per invocation; Spool and retention limits | fleet run 01 live bytes; history run 01 (≤ 480 MB), stress run 01 (≤ 398 MB) and soak run 01 (≤ 968 MB) peak live bytes under the 5 GiB runner limit; outage run 01 peak Spool 3.7 MB of 256 MiB. Retention limits are not reached in any run | Measured on `2b5c939` (not target profile) |
| QUAL-ACK | Delivery latency | ACK p99 ≤ 1 s | delivery run 01 (`71fef99`); fleet run 01; soak run 01 (`2b5c939`, four-CPU host): ACK p99 ≤ 63.6 ms in every 600 s window over 91 min | Measured on `2b5c939` (not target profile) |
| QUAL-FRESH | Freshness | observation to query p99 ≤ 5 s at 1,000 identities | history run 01 under revision 2, four-CPU host (`63bbeac`): p99 ≤ 1.78 s | Measured on `63bbeac` (not target profile) |
| QUAL-QLAT | Query latency | p99 ≤ 2 s over 1,000,000 records | history run 01 under revision 2, four-CPU host (`63bbeac`): p99 ≤ 481 ms Segments, ≤ 1,515 ms journal-only | Measured on `63bbeac` (not target profile) |
| QUAL-DRAIN | Outage drain | 30 min buffered, drained ≤ 10 min | outage run 01 (`bb8d06d`, four-CPU host): drain ≤ 99 s, oracle exact | Measured on `bb8d06d` (not target profile) |
| QUAL-INSTALL | Installation behavior | running-installation acceptance | [installation acceptance run 01](../experiments/formal/installation-acceptance-run-01.md) (`bbd2dd5`, container on a legacy cgroup hierarchy): 16 of 17 checks passed, three mutations rejected, `MemoryHigh` not run | Inconclusive |
| QUAL-H | Harness correctness | runner limits, rate oracle | test suites | Tested |

## Observation record (proposed, ADR-0023)

Claims of the [fabric-observation](../../crates/fabric-observation/src/lib.rs) codec. Nothing in the product depends on it; the claims are not in the registered contract list of `xtask/checks.json`.

| ID | Contract | Spec | Production implementation | Verification | Independent oracle | Negative control | Formal model | Fault test | Status | Unchecked |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| OBS-1 | Every valid block decodes to itself and has exactly one byte string | [ADR-0023](../decisions/ADR-0023-define-an-observation-record-with-a-canonical-encoding.md) | `fabric_observation::encode`, `decode` | `round_trip_all_three_signals`; property `round_trip_is_identity` (1,500 cases) | — | `rejects_non_canonical_records_before_encoding` (unsorted and duplicate keys, NaN, severity 25) | Kani harnesses for zigzag and delta (`cfg(kani)`, not in the registered check) | — | Tested | Blocks above 40 records in the property generator |
| OBS-2 | Every accepted byte string is the canonical encoding of its decoding, and no input panics the decoder | ADR-0023 | `decode`'s canonicality checks | `single_bit_flips_are_rejected_or_canonical`; properties `mutations_are_rejected_or_canonical`, `arbitrary_bytes_never_panic`; fuzz target `observation_block` (corpus replay in `fuzz-corpus`) | — | `rejects_overlong_varints`, `rejects_dictionaries_out_of_first_use_order_or_unused` (including [CX-FOB1-DUPLICATE-DICTIONARY](counterexamples.json), found by the mutation property), `rejects_crc_mismatch_truncation_and_trailing_bytes`; fuzz regression `duplicate-dictionary.fob` | — | — | Tested | Coverage-guided fuzzing beyond the committed corpus (needs the nightly `fuzz-smoke`) |
