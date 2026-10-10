# Production service pressure and recovery acceptance

Registered before the candidate burst cells. This supplies the finite pressure
and recovery portion of B6 in the [release plan](../../milestones/release-readiness.md).
The separately registered long outages, four cross-family cells and sustained
soak retain their own gates. This does not supersede historical stress trials.

## Candidate burst cells

Use the exact linked Debian/RPM candidate and matching qualification helpers,
production `serve`, real virtual-authenticator WebAuthn, a separately scoped query
credential, and the mandatory companion. Enroll 100 simulated identities during
unmeasured paced setup. Run each of the three frozen seeds `0xA11FA001` through
`0xA11FA003` for 180 seconds. The unchanged simulator supplies two 512-byte logs
per identity/second and 32 metric points per 15 seconds. From second 60 through
79, multiply log production by five. Retain every creation, attempt and answer.

Keep the real PWA visibly polling throughout. Every 0.5 seconds, request inventory,
change one seeded identity's metric interval between 15 and 30 seconds, and query
its previous 30 seconds of logs. Read every continuation page; any management or
query failure, incomplete answer or missing expected iteration is recorded.
The simulator's offered metrics remain fixed, as in the main trial.

Enroll ten extra probe credentials and revoke five before measurement. Starting
at second 50, perform 200 rounds spaced by at least 0.2 seconds: submit one
independently constructed Batch with a revoked credential, one with an unknown
credential, and one malformed body with an active probe credential. Revoked and
unknown credentials must return 401; malformed bodies must return 400 or 413.
No rejected source identity may appear in recovery. Preserve all statuses;
overload or a harness exception cannot count as the intended rejection.

After producers finish, require all admitted work to ACK within 120 seconds.
Stop the server/companion and inspect their durable state using source-matched
helpers. Independently inspect the companion Spool, join its actual delivery
hashes/answers, and include all source and recovery records in the unchanged
delivery oracle. Missing companion evidence or any duplicate/missing/changed
ACKed source fails the cell. Keep the simulator's original source/attempt ledger.

The burst cell passes only with exact oracle custody, all expected rejection
rounds, no rejected source committed, successful actual management/query/UI work,
clean process exits, and the following bounds. Outstanding count at second 175
must be at most the count at second 55 plus 100 Batches, and final outstanding
count must be zero. This is the historical stress rule scaled to the declared
100-identity release workload, not a new steady latency rule. Report complete
creation-to-first-ACK and first-attempt-to-ACK populations before, during and after
the burst, including retries and censoring; burst latency has no steady-service
threshold. An injected missing ACKed recovery record must be rejected by the
unchanged oracle before full execution.

Use the [local fixture's](release-service-query-protocol.md) 6 GiB outer/no-swap,
4 GB server-plus-companion subgroup, two server CPUs, 2 GiB server RSS gate,
1 GiB browser subgroup and 5 GiB combined live storage allowance. Count browser
temporary files outside the main fixture, all source archives and compact evidence.
Reserve below the 100 GB aggregate DATA ceiling. The launcher deadline is
1,800 seconds. A separate short smoke uses 10 identities for 12 seconds with a
two-second burst and ten rejection rounds; it exercises mechanisms only and
cannot satisfy a full cell. Preserve failure evidence and verify cleanup.

## Recovery and storage controls

Before full successor trials, the management generator uses absolute half-second
deadlines, starting one quarter-second after its local simulator-launch anchor.
This places attempts away from half-second bucket boundaries and avoids accumulated
sleep drift. The checker still uses the simulator's independently reported actual
begin time and requires coverage of every measured half-second slot. Record each
scheduled deadline, actual start and scheduling lag; startup delay or a missed
slot still fails. The earlier short smoke that missed slot 11 remains failed.

The source-matched candidate must run the following existing executable controls
with their independent fixtures and original outcomes. Record actual commands,
exit statuses and the source identities; a historical pass is insufficient:

| Required behavior | Existing control |
| --- | --- |
| Full Spool keeps custody and exposes unavailable coverage | `tests/spindle.rs::source_failure_and_full_spool_leave_visible_gap_or_unknown_coverage` |
| Scoped ENOSPC preserves unACKed bytes and requires explicit recovery | `tests/spool_enospc.rs::native_enospc_preserves_unacked_bytes_and_requires_recovery` |
| Sealer input/output failures preserve custody | `completion_storage::scoped_input_and_output_path_failures_keep_custody_and_retry_exactly` and `span_table_and_filter_write_and_sync_faults_keep_pending_custody` |
| Actual process kills across publication/checkpoint/reclaim preserve answers | `completion_storage::named_sealer_kill_cuts_recover_exact_custody_and_oracle_query_chains` and `an_actual_owned_child_killed_after_publication_reclaims_and_replays_exactly` |
| Configured byte and age rollover retain the exact expected suffix | `completion_storage::byte_ceiling_and_interrupted_retention_preserve_exact_suffix` and `age_eviction_grades_the_predeclared_retained_ledger_and_expires_pages` |
| Existing page snapshots survive publication and expire after retention | `query_snapshot_transition::straddling_publication_preserves_old_page_envelope_and_retention_expires_it` |
| TLS trust/SAN/auth refusals and retry survive real collector lifecycle | `native_lifecycle` integration target, plus exact-package rejection probes above |

These tests exercise owned state and named syscall/process cuts. They do not
claim a physically full host filesystem or physical power-loss testing. Retention
uses smaller configured ceilings inside the laboratory budget; package checks
separately require the shipped 100 GB retained-data default. Existing source
fixtures and independent query/delivery oracles remain unchanged. Confirm the
tests' Rust inputs match the frozen candidate source manifest before attributing
their results to this candidate. The full fast registry runs these integration
targets; retain its individual receipts instead of inferring execution from CI's
overall status.
