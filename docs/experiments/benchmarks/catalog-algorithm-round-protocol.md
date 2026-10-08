# Indexed commit planning, dense query selection and collection ownership

Registered before execution. Continue the [delivery/ownership investigation](catalog-delivery-read-findings.md)
under the existing frontier and evidence limits. This is finite algorithm work,
not deployment qualification, a new wire format, or a durability change.

## Competing representations

Three independent lab audits found:

1. `GroupPlan` appends a credential/Spindle pair for every accepted Batch, then
   scans all prior pairs while deciding each later offer. Repeated binding work
   can grow quadratically with group population. Candidate: one binding inline,
   with ordered indexes for additional distinct credentials/Spindles. Preserve
   the first staged credential binding, explicit durable-state precedence and
   every reverse binding, even with changing supplied durable facts. The inline
   first binding avoids allocating extra indexes for a one-Spindle group.
2. Query `Smallest` keeps a key heap plus an ordered map of row payloads. Candidate:
   keep the heap and store payloads in bounded dense slots, reusing the evicted
   slot. Heap entries contain key, admission order and slot. This removes a
   second logarithmic index without moving large payloads during heap sifts.
   Equal-key rejection and admission-order ties must remain identical.
3. Native log encoding copies body/path Strings into a temporary OTLP tree even
   though callers only need the final count. Move them instead. Also represent
   replacement counter history as `Option<History>`: an ordinary log-only or
   unsuccessful host-sample path does not need a deep copy of unchanged history.
   Replacement still occurs only after successful durable append.

An owning payload heap was considered for selection but moves larger entries;
dense slots keep heap comparisons/movement small. A plain pair of binding maps
would add allocations for the common one-binding group; the inline first binding
addresses that case. Chunk-scanning log input and allocation-free hex formatting
remain candidates for a later round rather than being coupled into these tests.

## Correctness and falsification

No product contract, oracle, negative-control policy, wire field, dependency,
core-purity rule or sync order changes. The pure core keeps `no_std`, no effects
and no unsafe code. Registered invariants are existing ADR-0013 delivery/binding
decisions, HIST-4/HIST-7 exact ordering/pagination, and atomic source cursor/
counter-history movement with Spool commit.

Commit planning: differential decisions against frozen original code for all six
decision outcomes, generations, retries, conflicts, stale/gap offers, binding
conflicts and changing explicit durable facts. Seed42 exercises64 histories of
512 offers. A2048-offer single binding retains no secondary index entries.
The truth-table and existing delivery oracle remain independent checks.

Selection: frozen original implementation plus a stable full-sort reference;
capacities1/21/1001, ascending/descending/equal/mixed keys, threshold after each
offer, payload identity and drop lifecycle. Incorrect threshold admission and
reversed equal-key ties must be rejected by the comparison. Existing complete
Scan/Walk query chains in the fast suite remain the integration oracle.

Collection: exact encoded bytes against frozen original encoder and explicit
field/ownership checks, including Unicode, empty text and numeric attributes.
Borrowed encoding must demonstrate distinct live allocations. Quiet/log-only
passes, failed host sampling and failed Spool commit retain counter history and
unadvanced cursors; successful metric commit replaces history. No advancement
based merely on readable source bytes is introduced.

## Finite performance screens

All fixture generation and result equality checks are outside measured regions;
retain raw timing rows, commands, source hashes, build profile and limitations.
Three pairs alternate arm order. `Instant` wall timings include the algorithm's
allocation/drop work where stated by each probe; aggregate cgroup CPU/RSS/I/O is
recorded by the launcher, not mislabeled as per-arm CPU.

* Commit decision probe:1/8/32/256/2048 offers, single-Strand and distinct-binding
  fixtures, `max(64,131072/offers)` plans per arm,131072 offers per arm. H1: each
  distinct-binding pair at256/2048 is at least20% faster; the median of three
  ratios at1/8 is at most1.10 for each shape. H0: any required criterion misses.
  A large single-Strand group is an algorithm stress case: normal one-request-
  in-flight delivery usually prevents that population in one server group.
* Selection probe:4096 log rows, body sizes16/1024, capacities21/1001, ascending,
  descending, equal-time and seeded mixed order; three pairs, five repetitions
  per arm. H1: median replacement-heavy (descending) ratios at most0.90 in each
  size/capacity cell; median low-churn ratios no greater than1.10. Report all
  shapes and retain any misses. This is selection cost, not full-query latency.
* Encoding probe:1024 lines with128-byte bodies (100 repetitions) and192 lines
  with4000-byte bodies (20 repetitions), short `fixture.log` paths; both fit the
  native Batch budget. H1: large-body median ratio at most0.95, small-body at
  most1.10. H0: either misses. Structural elimination of String copies can still
  be recorded when timing is inconclusive; do not convert that into a speedup.

Raw JSON rows and exact output comparisons are sufficient evidence for these
deterministic kernels; no large executable archives or generated datasets are
required. Source snapshots and locked Cargo builds reconstruct the probes.
Production adoption requires preserved semantics and review of both improvement
and small-workload costs. A missed timing guard is recorded and investigated,
not relabeled as passing or silently excluded.

## Execution and resource envelope

Only root launches workloads, serially, with the existing20 GiB max/16 GiB high,
no swap, data-drive build/scratch paths and1800-second outer deadline. No remote
work. Root's driver formats touched Rust, runs scoped controls, executes release
probes, hashes exact sources and records every exit. Maximum job900 seconds,
2 MiB prospective evidence reservation and1 MiB driver output cap. Full fast and
manual documentation checks follow in a separate contained job, also at most900
seconds and2 MiB reserve. About2969.73 seconds and7.7 MiB conservative query
evidence headroom remained before this round; neither budget is reset.

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-algorithm-round-01 --lab coordinator --stage preparation --seconds 900 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 2 -- python3 -B tools/bench/labs/catalog/algorithm_round.py
```

Successful owned scratch is removed. Failures retain exact output and fixtures
under the launcher's preservation workflow. Check closeout reuses the established
exact archive-reference cleanup for its Bun copy, so a documentation failure does
not silently consume another79.5 MB of temporary binary evidence.

## Separate progress question

Read-only inspection suggests that after committing the first oversized-line
gap, a skip-only continuation can repeatedly scan the same source interval until
a metric/other payload causes a commit. A3 MiB+10-byte line plus a normal suffix
would distinguish this from the existing shorter `collect_once` test. This is
currently an unrun hypothesis. Persisting cursor-only progress requires a
permitted Batch decision; do not hide that semantic choice inside copy removal.
