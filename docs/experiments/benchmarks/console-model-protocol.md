# Console model probe

Status: registered before the first probe execution on 2026-10-10. This is a
finite UI-model experiment, not release/browser/authentication acceptance.
The [algorithm model](../../architecture/console-algorithms.md) defines semantics.

Hypothesis: pixel-bounded extrema reduce the geometry passed to a chart while
preserving observed extrema and explicit gap flags. Null: geometry cannot be
reduced at the registered resolution, or the transform loses those properties.
Raw points and an extrema envelope have different display semantics; no claim of
full information equivalence or automatic end-user latency improvement follows.

Input: 65,536 monotonic nanosecond samples starting at
`1700000000000000000`, integer spacing 1 ns; ordinary values `i % 256`, a
10,000-unit spike at each `i % 512 == 511`, and a gap at each
`i % 1024 == 1023` (gap wins). Inclusive chart window covers first through last
sample. Compare raw non-gap point geometry with 256 independent envelope buckets.
Use fixed input, five warmups and 21 interleaved repetitions of both transforms.
Record p50/p95 CPU-path wall time, raw/output primitive counts and output-vector
logical byte estimates. Those byte estimates exclude allocator overhead and
input storage; they are not RSS/heap measurements. Keep source hash and command.

Correctness requires every non-gap value to fall inside its bucket's extrema,
every input gap to mark its bucket, ≤512 extrema, no connected/interpolated
geometry and exact timestamp preservation. Native boundary tests independently
cover nanosecond rounding, overflow, Unicode allocation capacity, retained-tail
eviction, stale-session output and polling pressure. Time measurements have no
post-hoc pass threshold. A slowdown in preparation must be reported even when
geometry shrinks. Browser paint/interaction and live-server behavior are separate.

Run under the resource launcher with two build jobs and the mounted data drive:

```sh
python3 -B tools/resource_group.py -- cargo run --locked --release -p fabric-ui --example model_probe
```

Use the configured data-drive Cargo/toolchain caches. Store compact results and
resource/cleanup receipts on that drive; research observations belong in the
[wiki](https://github.com/kmosoti/FabricO11y/wiki). No telemetry corpus is retained.

## Ordered-boundary ablation

Registered after the initial direct-division probe and before the candidate run.
Sorted timestamps permit a monotonic bucket cursor. For bucket j, the first
admitted integer offset is `ceil(j * span / P)`. Advance the cursor at each such
boundary; compute a boundary only when it changes. This substitutes at most P
wide-integer divisions for N divisions without changing the floor-formula result.
Null: the candidate changes any bucket/extremum/gap, or does not reduce preparation
time under this fixture. Keep the original per-sample division transform in the
probe as an executable reference. Compare complete output equality first, then
21 interleaved timings with alternating reference/candidate order after five
warmups. Report each median/p95 and actual logical output sizes; one host fixture
is not a universal UI speedup. Exhaustive small span/bucket boundary cases must
match direct integer division, including more buckets than time offsets.
