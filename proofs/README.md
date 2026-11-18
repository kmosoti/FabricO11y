# Admission proofs

`admission-kernel/src/lib.rs` is executable Rust used by the SDK adapter.
Verus checks its byte arithmetic, slot transitions and FIFO ticket operations.
Charon extracts that crate, Aeneas translates it to Lean, and
`ExtractedAdmission.lean` checks twenty-two named properties of the generated
definitions. No Lean FFI is used at runtime.

```sh
cargo xtask prove verus
cargo xtask prove aeneas
cargo xtask prove smt
cargo xtask prove telemetry
cargo xtask prove selftest
cargo xtask prove all
```

The kernel has private slot identity, capacity, encoded length and phase.
The production collection SDK embeds this non-Clone slot credit in each payload
buffer. Free/Reserved/Ready/Leased correspond to free/encoding/queued/in-flight;
retry changes Leased to Ready without changing identity, capacity or length.
Transitions preserve identity and capacity, reject invalid operations without
changing state, and bound encoded length. FIFO operations reject invalid tickets
and sequence overflow. The extracted conservation property describes the slot
phase accounting; connecting every concurrent adapter state to a whole-pool
formal invariant remains outside these proofs.

The Rust adapter supplies synchronization, buffer ownership and lease cleanup.
Its tests cover controlled interleavings, rollback, shutdown, poison, fixed
capacities, concurrent delivery and FIFO histories. Fuzz targets exercise
admission sequences, serialization limits and persisted framing against separate
reference models. These checks do not prove all thread schedules, allocator
behavior, I/O or whole-process memory bounds.

`Admission.lean` is a historical independent natural-number credit model checked
by `prove lean`; `admission.rs` is a historical standalone candidate. Neither is
evidence of extraction coverage or whole-SDK correctness.

See [native tool setup](../CONTRIBUTING.md#native-verification-tools) for pinned
checkers and dependencies. Commands record exact tool and source digests and
fail on missing tools, changed source, timeouts, opaque application functions or
unfinished proofs. The extracted theorem audit permits only `propext`,
`Classical.choice` and `Quot.sound` (or no axioms). The historical Lean model
permits `propext` and `Quot.sound`. Verus checks the application crate with
`--no-cheating`; upstream `vstd` specifications remain a trusted dependency.
Positive and deliberately false checker controls distinguish checker success
from unrelated compilation failure. Native checker acceptance does not prove
the compiler, extractor or checker implementation correct.

`admission-kernel/src/delivery.rs` supplies three additional executable SDK frontier
functions checked by Verus. `Collection.lean` adds thirteen named model theorems,
checked with `cargo xtask prove collection`, including an axiom audit and a rejected
skipped-receipt mutant. The transaction model assumes atomic storage; it is not a
proof of SQLite. See [collection verification](../docs/collection-verification.md)
for resource bounds, executable failure programs and the exact proof boundary.

`Distributed.lean` checks eighteen named custody-model properties with an axiom
audit and a rejected conflict-overwrite mutant (`cargo xtask prove distributed`).
Atomic stable storage and codec refinement are explicit assumptions; see the
[server evaluation](../docs/fabric-server-evaluation.md).

`Custody.lean` is an independent two-store protocol model with thirty audited
properties. It models exact byte lists, persistent source and retry-identity
bindings, fresh canonical IDs, uncertain upserts and publication after full
readback. Arbitrary transition traces preserve existing bindings, contiguous
visible source prefixes and allocation bounds. Additional properties cover
shadow-mode retention, receipt-scoped reclamation, authenticated lease expiry
without generation metadata, v2 exemption, generation retirement and cancellation
credit held until cleanup. Concrete negative controls reject conflict-overwriting
reservation and publication without readback. Logical bindings retain the original
bytes as specification state; this does not assert a second physical retained copy.
Atomic durable operations, receipt provenance, identity encoding, byte-faithful
runtime readback and the correspondence between cleanup and worker termination
are explicit refinement assumptions. Natural-number counters do not establish
Rust overflow handling. Filesystem, SQL engine, crypto, runtime synchronization,
whole-process memory and global optimality remain outside this model. The v2
constructor denotes unleased legacy data; rejecting lease-token downgrades remains
a runtime classification obligation.

`admission-kernel/src/telemetry.rs` adds Verus-specified executable predicates
(`watermark_advance`, `projection_frontier_advance`, `ring_can_buffer`, `disk_needs_eviction`, and
`rss_shedding_decision`) extracted through Charon/Aeneas and proved in
`ExtractedAdmission.lean`. The projection frontier accepts a range only when
its origin matches, publication completed, and its first ID immediately follows
the previous frontier. A failed range leaves all earlier commits intact. The Rust
callers establish origin identity, continuity within the range, and successful
publication. Native checker self-tests remove the publication gate from the
actual Rust implementation and require Verus to reject the mutant postcondition.

`TelemetrySmt.smt2` (`cargo xtask prove smt`) checks
64-bit Z3 `QF_BV` ZigZag bijection, modular Gorilla timestamp delta-of-delta
roundtripping, IEEE-754 XOR zero-trimming, and watermark monotonicity alongside a
saturating-delta mutant negative control. `Telemetry.lean` (`cargo xtask prove telemetry`)
checks nine Lean 4 theorems over Gorilla streams, Euclidean 5-minute rollup
alignment, and watermark traces alongside a premature-advance mutant control.
Unbounded `Int` proofs in `Telemetry.lean` model `align_rollup_bucket_ms` for
`ts >= -9_223_372_036_854_600_000` (`i64` timestamps whose aligned bucket fits in
`i64`; `[i64::MIN, -9_223_372_036_854_600_001]` saturates to `i64::MIN`).
Filesystem `fsync`/atomic rename durability and kernel `/proc`/cgroup sampling
accuracy remain explicit runtime assumptions.
