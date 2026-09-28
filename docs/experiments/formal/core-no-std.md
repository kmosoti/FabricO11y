# Experiment: a `no_std` semantic core

Status: executed on 2026-09-28 in the architecture-foundation milestone. This is a build-time experiment about which mechanism enforces core purity; it measures no performance.

## Question

Can `fabric-core` be `#![no_std]` with `extern crate alloc` without making the domain model worse, and which purity properties does that settle without a further check?

## Setup

`crates/fabric-core` with `#![no_std]`, `#![forbid(unsafe_code)]`, no dependencies, rustc 1.94.1. The delivery kernel (Strand identity, sequence rule, duplicate and conflict detection, credential binding, group overlay) was written directly in the crate; see [the delivery view](../../architecture/delivery.md).

## Results

| Probe | Command | Result |
| --- | --- | --- |
| Core compiles as `no_std` | `cargo build -p fabric-core --locked` | exit 0 |
| Clippy restrictions on non-test code | `cargo clippy -p fabric-core --locked -- -D warnings` | exit 0 |
| Negative control: `std::time::SystemTime::now()` added to the core | `cargo build -p fabric-core --locked` | exit 101, `error[E0433]: failed to resolve: use of unresolved module or unlinked crate std` |
| Escape hatch: `extern crate std;` plus the same clock call | `cargo build -p fabric-core --locked` | exit 0: **the compiler accepts it** |
| The same escape hatch under the purity gate | `cargo xtask check-core-purity` | exit 1, `PURITY_AMBIENT_SOURCE ... extern crate std` |
| Negative control: `ureq` added to the core manifest | `target/debug/xtask check-core-purity --declared-only` | exit 1, `PURITY_DENIED_DEPENDENCY fabric-core -> ureq: category=http-client` |
| Negative control: `fabric-server` added to the core manifest | `target/debug/xtask check-layers --declared-only` | exit 1, `LAYER_FORBIDDEN_EDGE fabric-core (core) -> fabric-server (composition-root)` |

Each injected change was reverted and both gates passed again. The two manifest controls ran the built binary directly because the `cargo xtask` alias passes `--locked`, and Cargo refuses to run while the manifest disagrees with `Cargo.lock`, which is itself a guard.

## What became unavailable

- `std::collections::HashMap`: the core uses `alloc::collections::BTreeMap`, which needs `Ord` keys. Strand identities are `Ord` byte arrays and integers, so this cost nothing and made iteration order deterministic.
- `std::io::Error`: decisions return domain enums instead. This is an improvement: the base code signalled a delivery conflict and a journal failure through the same `io::Error` type.
- Hashing: the core does not compute SHA-256. The caller supplies a `BatchDigest`; which hash identifies a Batch stays an adapter-support choice.
- Clock, environment, filesystem, network, process, threads: unavailable, as intended.

## Interpretation

Incidental failures: none so far; the delivery kernel needed no workaround. Real impurity exposed: the base commit path read `SystemTime::now()` inside the loop that also made delivery decisions; the extraction moved that read behind a `Clock` port. `no_std` does not by itself prevent `extern crate std`, global atomics or new dependencies, so the purity gate stays necessary. Maintenance cost: one extra attribute, `BTreeMap` instead of `HashMap`, and digests computed by callers.

## Limitations

Only the delivery kernel exists in the core today. Control, query, retention and collection rules have not been extracted, so this result says nothing yet about, for example, floating-point rate arithmetic in `no_std` (available in `core`, but `f64::powi`-style functions need `libm`). The source-pattern guard is syntactic and does not parse block comments or strings.

## Decision impact

[ADR-0016](../../decisions/ADR-0016-keep-a-pure-semantic-core.md) adopts `no_std` for `fabric-core`.
