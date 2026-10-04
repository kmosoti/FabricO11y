# Pending journal query screen

Registered on 2026-10-04 before execution, following the owner's request to
evaluate the append/rotate/seal path and query access to pending sealed journals.
Existing journal rotation is the appendable durable container; an unfinished
Parquet Segment is not introduced.

Use the ingestion-memory fixture at 65,536 shuffled 1,024-byte logs, seed 42,
128 logs per Batch, one sealed journal plus one small active-journal Batch.
Build the sealed file with the reference and bounded algorithms in three
fresh-process alternating pairs, one build worker. Query both scan and walk
before, during and after Segment publication, at the same fixed snapshot.
Retain actual answers and grade them against the unchanged Python oracle.
Include the active-journal Batch in expected custody and answer coverage.

Require exact ordered answers and completeness throughout, unchanged raw Batch
custody and no leftover private spill/build files. Report engine query wall time
by before/during/after and plan, sample counts, build wall/CPU and process peak
RSS. Do not label engine timings HTTP/consumer latency, or infer p99 from fewer
than 1,000 samples. Global allocation counts overlap concurrent queries and are
not a builder-only memory result in this population.

Request 20-row broad pages sequentially, alternating scan and walk, until the
build completes (at most 30 during samples, then wait). Queries before and after
also use these shapes. No artificial build delays, favorable retries or parallel
resource trials. If a build completes before a during query overlaps it, report
that coverage gap. One local CPU/IO domain remains a confounder; this is not
arrival-rate capacity, remote forwarding or deployment qualification.

Use the verified 20 GiB cgroup, no swap and the data drive. Preserve every
failure, all answers and command/source/binary hashes. Remove only owned trial
scratch after grading and archiving evidence; record cleanup and observations.

## Preflight grading correction and availability budget

The first small preflight failed because `query_oracle.check` expects a complete
page chain, while this screen samples first-page responses during publication.
Its failure is retained. Before any measured trial, grade each observed first
page against the unchanged oracle's `expected` rows (the first `limit` rows),
shape and envelope functions, and check that a continuation exists exactly when
more rows match. Do not fabricate unobserved pages or claim full-chain grading.
Negative controls must reject changed, missing and duplicated first-page rows.
Full-chain behavior remains covered by the unchanged HTTP/history integration
tests. Cache the independent expected answer per fixed query outside native
timings, to avoid repeatedly decoding the same input in the Python observer.

Owner clarification: up to 30 seconds after ACK is proposed experimental
processing headroom for query availability, not an intentional delay. This
finite screen tests already-committed journal/Segment visibility; it does not
measure ACK-to-first-visible latency or amend installed freshness promises.
