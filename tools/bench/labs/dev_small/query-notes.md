# Dev-small query measurement preparation

This note defines what the new observer can establish; it does not authorize
or report a workload. The coordinator owns M0/C1/Q1 execution and receipts.

## Evidence boundaries

- Reuse `tools/bench/observe_dev_small.py` for resource snapshots, HTTP query
  capture, visibility answers, and independent post-run Python query grading.
  The answer oracle remains `observation.native.query_oracle`; timed answers
  are compared against independently decoded `recovered.jsonl`, and quiescent
  pages are checked with `query_oracle.check`.
- Keep query-off sampling and final oracle checks. Its timed query and
  visibility fields must say `not_applicable`, with null values and zero
  samples. It does not establish visibility.
- Record the native phase clocks and a close observer clock triplet as integer
  fields (`wall_ns`, `mono_ns`, `observer_wall_ns`, `observer_mono_ns`,
  `boot_ns`) in each resource row. Gate the full sampled observer
  realtime-minus-monotonic and boottime-minus-monotonic ranges at 5,000,000 ns.
  Missing boottime makes timing ineligible. A failed
  guard invalidates timed phase claims while preserving exact correctness
  verdicts.
- Emit null for phase metrics with fewer than two samples, with the phase and
  reason retained. Do not turn an empty recovery window into zero CPU or a
  passing phase.
- Q1 visibility jitter belongs in selection of source tags, not solely in the
  retry poller. The private native harness should call the observer selector
  for each otherwise eligible tag, passing candidate identity/time and ordinal;
  selection decisions and the frozen schedule must be retained. Never rewrite
  source timestamps to imitate jitter.

## Q1 custody and query requirements

The inherited observer's `spool_cycle_custody` compares all recovered batches
with live cycle/ACK observations. A near-rotation fixture deliberately has a
historical population outside those observations, so applying that equality to
the combined population is the wrong partition. Preserve the full equality for
the live population, then independently verify the seeded historical identity,
sequence and hash ledger against recovered history. Do not drop old rows or
weaken the independent answer oracle. Keep seed and live custody results
separate in evidence.

The quiescent query set must include a bounded query for seeded historical
sentinels, in addition to recent live tags, absent text, and CPU metric history.
Grade its complete page chain against all independently decoded recovered
records. Retain oldest and newest seeded sentinels, missing/duplicate counts,
and any unresolved live probes. Query-before/during/after-seal answers remain
upper-bound observations; they do not identify internal publication time.

The private native harness calls `seed_info(root, seed_summary)` before startup.
The observer reads the independently generated `seed-ledger.jsonl`, queries
its oldest and newest source sentinels after quiescence, and preserves the seed
summary. Final custody accounting must partition the recovered replay into the
seed ledger and live source/ACK observations; the inherited combined-set gate
alone cannot represent the seeded condition.

The Q1 screen is diagnostic. Report each phase, seal-overlap status, query
latency, observation-to-answer distribution, unresolved targets, complete
snapshot results, and every gate separately. A build that completes between
visibility probes means overlap was unobserved. A 30-second diagnostic horizon
does not replace the registered five-second observation-to-query p99 workload.

## M0 control obligations

Before M0 timing, `controls()` must reject the archived Q3 clock-discontinuity
summary and its unavailable recovery phase, then reject deterministic injected
realtime jump, boottime/monotonic suspend gap, missing phase, rate mismatch, and
custody mismatch. It checks fresh-development observer configuration and the
deterministic selector against a simulated 180-second candidate stream; the
actual native fresh-development preflight is still an M0 workload result.
These are harness controls; they do not replace the independent delivery and
query oracles. Raw clocks and control outcomes remain retained with every
subsequent cell.

The archived counterexample is
`docs/experiments/benchmarks/data/readiness-labs-run-01/query/clock-discontinuity-fixture.json`.
The historical old protocol and oracle are frozen; this preparation does not
edit either.

## Retention and resource evidence

`observe_dev_small.Observer.finish` writes raw `resources.json`. For each lab
retention, gzip this file and remove the expanded copy only after successful
compression and hash/size accounting. Enforce a 50 MiB combined lab-evidence
bound, retaining compact summaries and failure evidence. Route all temporary
paths through `TMPDIR`/`FABRIC_SCRATCH_ROOT`; the coordinator must execute
workloads and validators through `tools/resource_group.py` and preserve its
receipt. Preparation alone has run no workload, test, or validator.
