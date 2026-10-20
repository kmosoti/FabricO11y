# Verification hash-backend ablation

Registered after the first complete native receipt experiment and before
dependency resolution, accelerated builds or measurements. Alpha acceptance
remains paused. The current scalar result stays preserved and is not relabeled.
The historical receipt driver and Python CLI referenced here are available in
the immutable source worktree documented by the
[receipt-verification protocol](verification-receipts-protocol.md#contract-and-primitives).
Reproduce this procedure only from that separate historical checkout; the
maintained branch intentionally retires those runtime files.

## Observation and hypothesis

At source `92d9bc4a784fa282ec803334ff7126c1cb6fbbf3`, both implementations
matched on 21 common cases, with four separately specified stricter Rust
refusals. Source, reference and ELF pins remained unchanged. Report
`results/rust-tooling-migration/receipt-candidate-01/candidate-report.json` has
SHA-256 `3a89529d38cb80cdd84590af9f24f391e428184a057ea4b65a8dd18a93a57341`.
Median Python/native wall times were 58.483/15.145 ms for 64-byte logs,
59.856/18.024 ms for 64 KiB logs, and 132.984/170.897 ms for ten 4 MiB logs.
The large-input Rust result is a performance regression, not a general speedup.

The measured CPU exposes AVX2 but no SHA instruction flag. The pinned sha2
0.10.9 source selects its software fallback on x86 without SHA extensions; its
optional `asm` feature replaces that fallback with sha2-asm. This makes backend
cost a testable explanation; filesystem/metadata cost is an alternative. Feature
availability is documented by [RustCrypto](https://docs.rs/crate/sha2/0.10.9/features).
Neither the language nor assembly feature guarantees a faster implementation.

## Controlled change

Add one nondefault xtask feature, `verification-sha-asm`, enabling the existing
sha2 0.10.9 assembly option. Keep the hash algorithm, digest bytes, file reader,
admission budgets, source queries, schema, independent reference and all controls
unchanged. Do not write cryptographic code, install GPU tools or alter product
packages. The feature is experimental and off by default during the comparison.

Resolve the optional dependency under containment, inspect the lockfile diff and
commit the exact resulting lock before builds. Preserve unexpected resolver
changes; do not silently update unrelated packages. The existing core purity
policy is unchanged. Later adoption needs ordinary dependency/layer checks and
a separate reasoned default/workflow change; this experiment is not adoption.

## Matched experiment

At one clean source with the same locked graph, build release xtask and the
existing receipt differential example both without the feature and with
`--features verification-sha-asm`. Archive each binary immediately with full
hashes before the other build can overwrite the shared target. Capture actual
feature commands and toolchain identity, not an unverified variant label.

Run the existing 25-case differential and all three size corpora for each
variant against the unchanged Python CLI. Each corpus has one warmup per
implementation and seven alternating pairs. Preserve raw outputs, actual GNU
time CPU/RSS, exact source and executable hashes, and per-run stop receipts.
Run the accelerated variant first to distinguish a missing/incorrect backend
quickly; rerun the default variant at the same head before comparing variants.
Compilation, fixture generation and binary copying stay outside trial clocks.
This controls the backend but does not prove absence of every shared-host effect.

Require identical accepted digest/summary semantics and all intentional refusals.
Keep any failed, unavailable or slower result. Prefer acceleration only if the
large-corpus median paired latency improves by at least 5% over same-head
default, with no material small-corpus regression or control mismatch. Report
comparison with Python separately; memory, CPU and wall time are distinct.

The full-byte hashing lower bound remains Omega(B), with a bounded read buffer.
This investigates a constant factor while preserving every input byte/check.
It does not certify arbitrary code, establish a global optimum, replace required
CI checks or authorize release/tag publication. Use the original resource
launcher and exclusive engineering scheduler; all caches/output/scratch remain
on DATA with an explicit fresh reservation and original enforced caps.
