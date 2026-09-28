# ADR-0016: Keep the semantic core pure, deterministic and `no_std`

## Status

Accepted on 2026-09-28 under the owner's architecture-foundation authorization. The `no_std` experiment is recorded with the first semantic extraction.

## Context

Delivery, control, query and retention rules decide what telemetry means. At the base they were computed next to `SystemTime::now()`, file writes and HTTP answers, so a test of a rule had to run a server, and a change to a rule could silently start depending on the clock or the environment.

## Decision

`fabric-core` holds domain decisions as pure functions: for identical explicit inputs they return identical outputs. A decision must not read the wall clock, environment variables, working directory, filesystem, network, process ID, locale, randomness, thread scheduling or global mutable state unless that value is an explicit typed input. Effects are returned as data (`DeliveryDecision::Commit`, a future `RetentionDecision::Delete(segment)`), and the application or an adapter performs them.

Mechanisms, each for a distinct gap:

- `#![no_std]` with `extern crate alloc`: the standard library's clock, environment, filesystem, network, process and thread APIs do not exist in the crate. Restating them as Clippy `disallowed_methods` would duplicate the compiler.
- `#![forbid(unsafe_code)]`: no FFI calls into the operating system.
- `cargo xtask check-core-purity` with [`docs/architecture/core-purity.json`](../architecture/core-purity.json): an explicit dependency allowlist (empty today), named denied categories for clear failures (async runtime, HTTP, TLS, network, filesystem, columnar storage, randomness, ambient clock, OS process, systemd), no build script, no unreviewed features, the two attributes above present, and a syntactic guard against `extern crate std`, `static mut`, `thread_local!` and static atomics. With resolved metadata it also follows the transitive closure.
- Clippy in non-test core code denies `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`, slice indexing and unchecked arithmetic. Decisions return explicit outcomes; a panic would be a failure mode outside the decision algebra, and unchecked `+ 1` on a sequence is exactly how a Strand's last sequence could wrap. Tests may panic.

Purity does not decide ownership: a CRC, a protobuf codec or a Parquet schema conversion can be pure and still belong to adapter support. A port is an effect boundary, not a pure function.

## Alternatives considered

- Purity by convention and review only. Nothing would stop the next `SystemTime::now()`.
- Clippy `disallowed_methods`/`disallowed_types` on a `std` core. Works, but lists only what someone remembered; `no_std` removes the whole surface.
- Crate-name substring matching as the only purity check. Easily evaded by renaming; the gate uses Cargo's package names, an allowlist and the resolved graph.

## Evidence

Fixture tests in [`xtask/tests/gates.rs`](../../xtask/tests/gates.rs): core depending on `rand`, `tokio`, a renamed `ureq`, an unreviewed crate, a build script, a feature, a missing `#![no_std]`, `extern crate std`, a static atomic, or a wrapper crate that links `tokio` each fail with a named category; an allowlisted pure dependency passes.

## Consequences

Easier: exhaustive and property tests of decisions without I/O; decisions are reusable by the server, the Spindle and offline tools. Harder: `HashMap` is unavailable (the core uses `BTreeMap`, which needs `Ord` keys); errors are domain enums rather than `io::Error`; a digest or timestamp must be computed by the caller. New constraint: adding a core dependency is a reviewed change to the purity policy.

## Validation

Falsified if a core function returns different results for identical explicit inputs, if any fixture defect passes the gate, or if a core decision needs an ambient input that cannot be made explicit.

## Related

[ADR-0015](ADR-0015-adopt-a-hexagonal-architecture.md).
