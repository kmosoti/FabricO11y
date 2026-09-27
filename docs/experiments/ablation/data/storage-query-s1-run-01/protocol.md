# S1: Optional summaries over a replayed event snapshot

## Registration

Registered before implementation selection and measurement on 2026-09-27. This protocol is frozen separately from the result record. The branch starts at `40fa467` on updated `main`. The research question is whether conservative summaries reduce rows inspected for selective queries while returning exactly the same ordered results as a full scan. This is a CPU and logical-work experiment over memory after replay, not a disk-I/O or storage-format comparison.

## Fixed contract

Use the existing `EventLog` and its two-sync append unchanged. Reopen each fresh log and verify every replayed event against its deterministic source using `same_record_contents`. Move the replayed events into an immutable snapshot in a separate research package. An index cannot outlive or mutate independently of its owned rows. Result identity is the row position within that snapshot: repeated `EventId` values must not collapse results.

A query is an inclusive event-time interval `[start_ns, end_ns]`, optional exact tenant ID, and optional exact, case-sensitive log token. Tokens are defined by Rust `str::split_whitespace`; punctuation remains part of a token. A Gauge never satisfies a token predicate. A reversed interval returns no rows. Both variants apply the same exact predicate to retained candidate rows and return positions in input order. This is not substring, prefix, language-aware full-text, or SQL search.

Compare `Scan` with `Pruned`. Split arrival-order rows into blocks of 64, including a short final block. Summaries contain the actual minimum and maximum time and separate tenant/token Bloom filters of 2,048 bits each with three deterministic hash probes. Summaries do not assume time ordering. Missing summaries always scan. There is no on-disk index, background writer, hot/cold movement, or change to application visibility and durability.

## Workloads and queries

Use seeds `42`, `43`, `44`, 2,048 events per workload, and the existing seeded Gauge generator as the event envelope. Experiment-only transformations are deterministic:

| Workload | Payload, time and tenant distribution |
| --- | --- |
| `gauge` | Unchanged generated Gauge events and four tenants |
| `clustered_logs` | All Logs, tenant `index / 256 + 1`, ascending generated time |
| `shuffled_logs` | All Logs, tenant `(index * 17 + seed) % 1024 + 1`; time permuted by `(index * 109 + seed) % 2048` milliseconds from the first time |
| `mixed` | Even positions become Logs, odd positions retain Gauges; tenant `(index * 17 + seed) % 1024 + 1`; ascending time |

Log body: `common service{index % 16} request{index}` plus `rare` for `index % 257 == 0`, else `normal`. Append 128 `x` characters as a token to make the raw body less trivial; this is synthetic repeated text, not a production compression corpus.

Generate 128 queries, cycling through eight cases 16 times. Case 0: entire time range, no other predicate. Case 1: a 32-millisecond window whose start is `(cycle * 127) % 2048` milliseconds after the first time (inclusive end at start + 31 ms). Case 2: entire time range and tenant `cycle + 1`. Case 3: entire range and token `rare`. Case 4: token `absent`. Case 5: token `common`. Case 6: the narrow time window, tenant `cycle + 1`, token `rare`. Case 7: reversed interval. Keep the exact same queries and rows for each variant. Boundary, Unicode, duplicate-ID, unsorted-time, unavailable-summary and saturated-filter cases belong in correctness checks, outside timing.

## Metrics and boundaries

For each workload/seed: record durable write wall time from first append through last append (generation excluded), raw file length, reopen plus replay/verification wall time, summary build wall time, logical summary bytes, and process peak RSS where Linux exposes it. Logical summary bytes count only two fixed bitsets and two `i64` bounds per available block: 528 bytes/block; they exclude container overhead, raw rows, strings and allocator overhead. They are not serialized index size or storage amplification.

Warm up all queries in both modes once. Then execute five trials of all queries, alternating mode order by `(trial + query_index) % 2`. Record each query latency in nanoseconds, candidate rows inspected, blocks skipped, match count and exact result equality. Timing includes predicate evaluation and result-vector allocation; it excludes oracle comparison, CSV output, construction and disk I/O. Keep every timing sample. Compute nearest-rank p50/p99 over each trial's 128 queries, then medians across trials. This mixed query p99 is not a single query's latency distribution. Report per-case row reductions and both modes' latency without selecting a latency winner from this small, warm-cache run.

The full benchmark runs in one process. Peak RSS is a cumulative process high-water mark, not isolated index memory. CPU seconds and peak RSS from the runner cover the entire benchmark child process, including append/replay. No per-query CPU, device IOPS, physical bytes read, network overhead, or time-to-searchable distribution is measured in S1. Candidate rows are logical work, not bytes read from storage.

## Decision rule and checks

Any mismatch against the independent brute-force evaluator, failed round-trip comparison, missing/reordered/duplicate output row, or fallback omission invalidates the run. Inject a defective pruner that drops a known matching block; the contract suite must fail. Inject a mismatch in a copy of the measured CSV; the result analyzer must reject it.

Advance to an on-disk block experiment only if all correctness gates pass and `Pruned` inspects at most 50% of `Scan` rows in each seed for the narrow-time case on `clustered_logs` and the absent-token case on `clustered_logs`. This only establishes useful logical pruning on named cells. Report the shuffled-time and common-token countercases even if they show no gain. A failed performance gate means revise the hypothesis, not tune these settings after looking at results. No result selects Parquet, a database, or a production index.

Before measurement, run the root tests and independent research contract tests. Preserve exact commands, source hashes including this preregistration, toolchain, kernel/CPU/filesystem, build settings, raw CSV, computed summary and SHA256 manifest. Use a fresh output directory; refuse to overwrite an earlier run. The runner and result record will provide reproducible commands.
