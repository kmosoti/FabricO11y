# Cheaper exact snapshot evidence

The boundary query now reduces supported timestamps without constructing query
Rows and borrows raw records from the storage reader. Every raw Batch still
passes its digest check, including excluded groups. Included payloads still
undergo full typed decoding. Failed scans do not publish partial metadata.
The fully covered Segment path continues to borrow manifest evidence.

The useful finding is twofold: eliminating row construction saves substantial
decoding-stage work, but the benefit is diluted in the complete raw-reader path; removing
raw copies then gives an exactly predicted allocation improvement with little
additional time benefit. The original two-cell 10% speed hypothesis was **not
confirmed** by its second run. This investigation does not establish HTTP query
throughput, lower service RSS, a cold-reader result or deployment qualification.

## Row construction: screen and confirmation

The [first protocol](catalog-range-evidence-protocol.md) fixed four mixed-signal
fixtures, three alternating pairs and 16 repeats. Both arms used the same public
raw reader with its default Arrow batch size. Each Entry had four logs, four
Gauge/Sum points and two spans, plus an unsupported Histogram. The direct
reducer retained all typed validation but avoided row vectors, attribute maps,
identity/name copies and span hexadecimal formatting.

Percentages below are medians of paired reductions, not ratios of independently
selected median times. Raw samples and all paired CPU ratios are in the linked
summaries.

| Cell | Raw bytes / included observations | Screen decode time saved | Screen scan time saved | Production-function confirmation scan time saved | Scan requested bytes saved (both runs) |
| --- | --- | ---: | ---: | ---: | ---: |
| A | 52,704 / 80 | 45.2% | 16.4% | 22.2% | 17.3% |
| B | 306,704 / 80 | 41.9% | 4.9% | 5.9% | 5.1% |
| C | 1,226,816 / 320 | 42.5% | 10.7% | 7.7% | 4.2% |
| D | 1,226,816 / 560 | 41.1% | 10.8% | 9.2% | 6.3% |

The [screen](data/catalog-range-evidence-screen-01/summary.json) met its
nomination rule. The [confirmation](data/catalog-range-evidence-fixed-01/summary.json)
did not: only A exceeded 10%, although every timing/allocation guard and exact
answer succeeded. The execution receipt's exit 0 records a successfully
completed comparison and its checks; it does not turn `nomination: false` into
acceptance of the speed hypothesis. The results remain unchanged.

Whole-scan allocation calls fell 45–48%, but incremental peak requested heap
fell only 0.2–5.1%. That is mainly reduced allocation churn. The public reader's
small-fixture results must not be described as measurements of production's
one-row reader or a guaranteed end-to-end query speedup.

## Borrowing: a prediction checked byte for byte

The [second protocol](catalog-range-borrowed-protocol.md) tested a different
mechanism with matched **one-row readers**. The owned baseline already used
direct timestamp reduction and moved its Entry label into the map. The candidate
borrowed raw fields and allocated a label only for a new freshness key. Five
alternating pairs ran 64 scans per arm; a separate counted build measured heap
requests. It introduced no replacement speed target.

For N nonempty raw records and K distinct included labels with supported
observations, the predicted saving per scan was:

`requested bytes = sum(all Batch lengths) + sum(all label UTF-8 lengths) - sum(K label lengths)`

`allocation calls = 2*N - K`.

| Cell | Exact bytes eliminated per scan | Exact allocation calls eliminated | Requested bytes saved | Median wall-time change |
| --- | ---: | ---: | ---: | ---: |
| A | 52,794 | 31 | 9.6% | 1.1% lower |
| B | 306,794 | 31 | 15.7% | 0.6% lower |
| C | 1,227,194 | 127 | 16.9% | 0.8% lower |
| D | 1,227,152 | 120 | 13.7% | 0.9% lower |

Every counted pair matched both predictions exactly. Peak extra requested heap
was identical, and every median wall/CPU non-regression guard succeeded.
The roughly 1% timing differences are descriptive; no material speed gain is
claimed. The [summary](data/catalog-range-borrowed-01/summary.json) admits the
copy-elimination mechanism under its own preregistered allocation objective.
Do not multiply these savings by those of the previous default-reader trial.

## Implementation and correctness evidence

Storage owns [`RecordRef`](../../../crates/fabric-server/src/segment.rs): its
label and bytes borrow the current Arrow batch through a higher-ranked callback.
The common reader verifies each raw Batch digest before calling the visitor.
Existing owned readers wrap that same implementation; their default batch size
and error order remain intact. The explicit owned/borrowed one-row APIs have
concrete probe/query consumers. No unsafe production code or new dependency was
introduced. Record digests cover Batch bytes, not independent label/group/time
columns, and a borrowed view grants no retention lease.

[`latest_observation_bytes`](../../../crates/fabric-server/src/rows.rs) decodes
the same Batch and full OTLP types in the same order. Logs contribute observed
times; Gauge/Sum points contribute point times, even with an absent value; spans
contribute start times. Unsupported metrics and gaps contribute no freshness.
`None` and a real timestamp of zero remain distinct. The Entry-based API delegates
to this reducer. [`boundary_evidence`](../../../crates/fabric-server/src/query.rs)
stages all values until the raw scan completes, retaining existing error behavior.

Each of the first two runs recorded 96 known-output checks and 128 semantic
controls across plain/counted builds; the borrowing run recorded 80 known-output
checks and 136 controls. Controls include malformed and ignored nested fields,
decode precedence, empty/middle ranges, missing/corrupt projections, and a bad
digest outside the snapshot. The borrowing run also records N-1 successful
callbacks before a final corrupt record rejects all staged evidence. Unicode
labels and unequal payloads copied out of callbacks survive reader advancement
and destruction. Numerical grading rejects 12 injected defects in each original
run and 14 in the borrowing run; archive controls reject altered, missing and
duplicate members. These are finite executable checks, not general proofs.

The integrated candidate ran four independent known-value timestamp tests, the
fully covered/corrupt boundary control, and the unchanged publication/snapshot
integration test: ten complete oracle-graded chains, two missing-row rejection
controls and two retention Gone checks. The same six Rust tests exited 0 after
both integration steps. The Python query oracle was unchanged.

## Reproduction, resources and cleanup

Dispatch each registered ID through `python3 -B tools/resource_group.py --`,
then `completion/run_job.py`, `catalog/coupled_admit.py`, and
`catalog/range_evidence.py --mode screen|fixed|borrowed|checks`. Exact argument
vectors, source archives/diffs, source/binary/stdout hashes, resource samples,
commands and exits are in the coordinator and driver receipts.

| Job | Exit / elapsed | Cgroup peak including compilation | Preserved decoded fixture files |
| --- | --- | ---: | ---: |
| [screen](data/lab-completion-run-01/coordinator/catalog-range-evidence-screen-01/receipt.json) | 0 / 32.600 s | 1,127,845,888 B | 430,476 B |
| [production-function confirmation](data/lab-completion-run-01/coordinator/catalog-range-evidence-fixed-01/receipt.json) | 0 / 44.184 s | 2,097,270,784 B | 480,724 B |
| [borrowed comparison](data/lab-completion-run-01/coordinator/catalog-range-borrowed-01/receipt.json) | 0 / 71.967 s | 12,322,566,144 B | 639,498 B |

The last job also compiled workspace all-feature test artifacts with `--no-run`;
that command is build evidence only. All jobs used the mounted data drive with
16 GiB high/20 GiB max and zero swap; no high/max/OOM event occurred. The peaks
include compiler and page-cache charges and are not application RSS. Every
fixture archive was compared byte for byte with the originals before owned
scratch was removed. No time or evidence budget was reset.

The [verification job](data/catalog-range-evidence-checks-01/receipt.json) ran
all 17 fast gates successfully (profile exit 0), including workspace tests,
Clippy, independent oracles, properties, fuzz corpus and network simulation.
The documentation content check also exited 0. The coordinator then hit its
320-second deadline during documentation-checker self-tests; hook tests did not
run. Its combined result remains timeout/exit -9, not passed. The preserved
launcher tree contained twelve files totaling 412 bytes.

A [separate budget amendment](catalog-range-closeout-protocol.md) transfers one
unused preparation minute to verification, preserving their aggregate allowance
and all previous consumption. The
[closeout receipt](data/catalog-range-closeout-01/receipt.json) records exact
preservation/removal of that tree, byte comparison of the implementation with
the successful fast run, and another manual documentation run. That run exposed
an evidence-packaging error: loose Markdown snapshots were checked as live
documentation and their relative links failed (166 issues). Checker probes and
hook tests passed. The [packaging repair](catalog-range-closeout-retry-protocol.md)
archives those copies with exact byte readback and removes only the loose
snapshots; future copies use `.txt` suffixes. The final unchanged manual profile,
source comparison and cleanup are recorded in the
[second closeout](data/lab-completion-run-01/coordinator/catalog-range-closeout-02/receipt.json).
The failed run remains recorded. No product test, oracle or performance
acceptance rule was changed to obtain another outcome.

## What the model changes next

Write `T = read + hash + copy + typed_decode + row_projection + reduction`.
The measured interventions remove row projection and raw copy independently.
Increasing raw bytes 5.82 times between A and B, at fixed observation/node
counts, diluted the first improvement. Removing the second cost changed exact
allocation demand without materially changing time or peak. Those observations
argue against promising another large gain from allocation tuning alone.

Next distinguish Parquet decoding, SHA verification and full typed decoding
under realistic repeated-page and mixed-ingestion load before changing another
algorithm. Reusing a verified range is not yet justified: a descriptor cache
cannot hide same-size raw corruption or outlive retention. Any future range map
must carry explicit freshness, integrity and lifetime rules. Human and automated
clients benefit only if the complete answer envelope remains exact.
