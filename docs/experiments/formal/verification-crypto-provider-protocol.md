# Verification crypto-provider discrimination

Historical reproduction of the Python reference and differential driver uses
the immutable source worktree documented by the
[receipt-verification protocol](verification-receipts-protocol.md#contract-and-primitives).
The maintained branch intentionally removes those runtime files; use a
separate historical checkout for the procedure below.

Register before implementation/builds. The previous ablation at clean source
`fd4870db817b611e303cbbb6413a98adaf796f92` preserved all 25 decisions in both
variants. Assembly/default native medians on ten 4 MiB logs were
154.418/170.036 ms, a 9.185% ratio-of-medians reduction. Assembly still had
12.42% higher median paired latency than its Python reference. The backend
batches ran in fixed order, not interleaved; each had seven alternating
Python/native pairs. Comparison report SHA-256 is
`b0188633b60d9cb33d5ffd08a7f8c71681eca2cc0cf1d19aec9291c073a61811` at
`results/verification-hash-ablation-01/actual-comparison-01.json`.

## Next discriminating change

The CPU backend affected large-input cost, but does not fully explain the
remaining regression. Compare an optional Rust OpenSSL SHA-256 adapter using
the already locked openssl 0.10.81 package against the existing default. Python's
hashlib uses its OpenSSL provider, so this separates a crypto-provider difference
from filesystem/metadata admission overhead more directly. This is a hypothesis,
not a claim that either language or provider is intrinsically faster.

Add a nondefault `verification-sha-openssl` feature and a small concrete
incremental digest adapter shared by the xtask one-shot helper and bounded file
reader. It must preserve SHA-256 bytes, update order, complete input reading,
byte budgets, no-follow paths, source checks and every refusal. No crypto
implementation, plugin/trait framework, product change or GPU work. Pure-Rust
sha2 remains the normal nonfeature path. Enabling both experimental features
must have explicit deterministic provider precedence rather than breaking
all-features checks. Do not make native crypto a new prerequisite for ordinary
xtask architecture/purity commands by changing defaults.

OpenSSL and Python share a provider family, so their agreement alone is not an
independent cryptographic oracle. Add known SHA-256 vectors plus cross-checks
against the existing RustCrypto implementation over deterministic sizes and
chunk boundaries, including 55/56/63/64/65 bytes and large input. Keep the Python
program unchanged as the independent receipt-semantics reference. Compare
provider/link versions explicitly; do not assume the processes load identical
libraries merely from their names.

## Execution and decision

Inspect and commit the lock change after contained dependency resolution. Only
the xtask edge to the already locked optional package is expected; reject and
preserve unrelated upgrades. At one clean head, run primitive/regression tests,
Clippy and formatting checks under the feature, then archive the release native
and experiment binaries with hashes. Run the existing 25-case differential and
three-corpus/seven-pair benchmark. Rebuild/archive/run the default at the same
head before making a provider comparison. Fresh output paths keep every failed
or slower result, including all previous scalar and assembly results.

Prefer the optional provider only with identical decisions/digests and at least
5% lower median paired large-corpus wall time than same-head default, without
material small-input regression. Report comparison with Python separately and
keep CPU/RSS/elapsed metrics distinct. If the provider does not resolve useful
performance, investigate actual file/metadata costs before adding another
backend. No global optimum or certification is claimed. Full-input hashing
remains Omega(B); improvement must not skip work or weaken a verification gate.

Use the original launcher, mounted DATA storage, original caps and exclusive
engineering scheduler, with an explicit complete reservation. Alpha remains
paused. Adoption, required full checks and the meaningful hash-comparison
checker mutant remain subsequent work, not credited by this protocol.
