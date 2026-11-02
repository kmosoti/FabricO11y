# Admission proofs

`admission-kernel/src/lib.rs` is executable Rust used by the SDK adapter.
Verus checks its byte arithmetic, slot transitions and FIFO ticket operations.
Charon extracts that crate, Aeneas translates it to Lean, and
`ExtractedAdmission.lean` checks fifteen named properties of the generated
definitions. No Lean FFI is used at runtime.

```sh
cargo xtask prove verus
cargo xtask prove aeneas
cargo xtask prove selftest
cargo xtask prove all
```

The kernel has private slot identity, capacity, encoded length and phase.
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
