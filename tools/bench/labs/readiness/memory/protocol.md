# Memory/shape screen, revision 1

Registered before implementation or execution. State: **not run**. Applies the
[fabric-experiment skill](../../../../../.agents/skills/fabric-experiment/SKILL.md)
and the [campaign plan](../../../../../docs/experiments/benchmarks/dev-small-readiness-plan.md).
This is a finite diagnostic screen; it does not complete BS acceptance.

H1: at seed 0xA11FA001 the production bounded builder holds the same ordered
rows, raw Batch bytes and manifest metadata as `segment::build` on four 64 MiB
inputs while its incremental live heap stays <=80 MiB and leaves no scratch.
H0: any differing decoded table/order/custody/metadata, heap exceedance, leftover,
missing output or nonzero subprocess exit rejects this screen. A separate
steady256 screen checks <=10% growth relative to steady64. Time is reported,
not gated. One process per builder per cell; no statistical performance claim.

Fixed contract: persisted formats, row ordering, query semantics and sync/ACK
behavior. No production code or independent oracle changes. The reference is
whole-file `segment::read_sealed` + `segment::build`; the candidate is
`segment::build_sealed`. Each process generates the same journal incrementally,
drops fixture allocations, then resets the counting allocator before reading
and building. Counters stop before read-back verification. Whole-process RSS
includes generation and verification; it is not builder heap. CPU/IO deltas
come from /proc/self/stat and /proc/self/io around build where supported.

Reconstructed fixtures: 100 labels, 64 logs per Batch (two per second over 32s),
32 gauge points and one span per Batch; a gap each 500th Batch. Steady alternates
512-byte repeat/entropy bodies; timestamps ascend per node/cycle. Outage makes
20 labels' timestamps 240s late for arrivals from240 through480s. Adversarial
assigns each log/metric an independent seeded timestamp in a520s window.
Bigrows uses32 logs per Batch with16384-byte bodies; node-local sequence is contiguous.
Every encoded Batch must be <=1MiB and Group <=4MiB before append.
These are legal sealer fixtures, not a native Spindle traffic replay. All use seed2703204353 (0xA11FA001); encoded Group
bytes reach target MiB with <=one Group overshoot. Frame-log bytes are reported
separately. These are intentionally not a claim to reproduce the study's exact
outage fleet scheduling or metric interval. Bigrows is the missing byte-cap
regression. Shapes and 256 scaling provide discriminating early evidence.

Checker compares ordered per-table SHA256 ledgers (logs, metrics, spans JSON
with float bits; gaps Debug fields; batches framed encoded Entry and Group ID),
manifest excluding file hashes/bytes, and logs/metrics physical hashes except
bigrows where byte-capped groups differ. This is differential evidence, not an
independent query oracle. Each file is authenticated by segment::verify.
Negative controls: changed row digest, changed manifest metadata, >80MiB heap,
leftover run and >10% growth must be rejected; unchanged sample is accepted.
The watchdog inventory includes a deterministic enumerated-run rename/delete
counterexample: missing old paths are tolerated while PermissionError must
propagate. Canonical file equality covers logs/metrics only; spans/filter files
are authenticated by verify and spans are compared as ordered decoded rows.
No failure injection or durability claim belongs to this screen.

Runtime: each pair <=300s per subprocess; four-shape job <=1500s; scale job<=900s; launcher high
16GiB/max20GiB/swap0/deadline30min, CPU allowance inherited from launcher.
Scratch only FABRIC_SCRATCH_ROOT/memory-<job>, requiring >=4GiB free; 4GiB
scratch ceiling is watched every100ms. Evidence destination must be absent.
Build uses launcher CARGO_TARGET_DIR on mounted data drive. Successful trial
state is deleted after compact JSON evidence. Failed state remains in launcher
scratch and is archived by launcher failure handling; runner records path and
never deletes failures. No fallback path. Compact evidence limited50MiB.

Unknowns: exact registered generator, 3 repeats,16/32/128MiB sizes, pruning
amplification, spill-byte attribution, injected failures/crash stages, concurrent
commit/query interaction and soak remain outside this wave. Cgroup resource
observations/receipts belong to coordinator; unavailable fields must be null.
Protocol hash and relevant source hashes freeze content in lieu of a missing
Git policy commit when metadata remains unwritable.
