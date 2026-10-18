# Console product source and harness identity

Prospective registration, 2026-10-18. Alpha acceptance is owner-paused while
verification engineering continues. This protocol changes no product package,
oracle, acceptance gate or release authority.

## Retained counterexample and invariant

The second Chrome baseline stopped before browser launch at harness `e08caaaa`.
Its resource record is `evidence/fabric-work-c1f2e977679b43a3831cdb952cca4c74`;
exit 1 and whole-service stop were recorded. `stage_console.py` compared the
frozen console receipt to its own newer harness checkout. The receipt's
`Cargo.lock` SHA-256 is
`1bec3939e821e84d60dd4a5b532e6f2acabaee309ee46aee68c5f16532c2496e`;
the harness has
`cf5160aab5c004c5b98c1f9bd8a7d28699ad01b89790fb7c3f2ebe830a61efcd`.
The source change adds tooling dependencies. This is a provenance failure, not
a browser result. Preserve the failed attempt and original source.

Frozen product source and executing harness source are separate authorities.
Every build-receipt source must still match its independently pinned product
commit. The default development stager must keep rejecting a stale build against
its current checkout. Never accept the receipt's own `source` field as authority,
skip `Cargo.lock`, rebuild the frozen package, or weaken asset verification.

## Bounded Rust primitive

First implement a read-only `xtask console-source` command taking explicit
`--source-root`, `--source-commit`, `--build-root` and `--receipt-sha256` pairs.
Unknown, repeated, missing or malformed arguments fail closed. Require actual
launcher containment and DATA storage. Pin the complete build receipt externally;
reuse strict JSON and the descriptor-owned `ArtifactStore`, including full-byte
digests, no-follow traversal and final identity rechecks. Require integer exit 0,
a nonempty source inventory of at most 512 canonical relative paths with SHA-256
identities, and a clean source checkout at the exact 40-character commit before
and after verification. Require every inventoried path to be tracked. Bound and
reap Git subprocesses; do not invoke a shell or accept receipt-selected programs.
Return structural identities, counts and hashed bytes, never acceptance credit.

Reuse existing byte/JSON ceilings without raising them. Source paths are indexed
once, each admitted source is hashed once per invocation, and subprocess count
is independent of inventory length. With N paths and B source/receipt bytes,
full integrity requires Omega(B) byte work; ordered inventory operations may add
O(N log N). Git repository inspection has separate repository-dependent cost.
No claim of universal optimality or adversarial immutable snapshot is made.

## Controls before adoption

Run a deterministic Rust-driven fixture against a locally committed product
source and a different clean harness commit. The current stager must first
reproduce the observed refusal with the original diagnostic. The new primitive
must accept the explicitly pinned original product and reject: wrong commit,
changed tracked source, forged source digest, changed receipt, source symlink,
untracked inventoried source, duplicate JSON keys, unsafe paths, missing source,
failed/boolean build exit, excess inventory and duplicate CLI flags. Inject a
representative skipped-source-hash defect and require the same corruption
control to reject it. Retain command, revision, output and launcher stop records.
Product delivery/query/rate oracles remain untouched.

Only after these controls may the legacy packaged-browser adapter select an
explicit source root after executing a caller-pinned native checker against the
caller-pinned receipt and commit. Keep default staging unchanged and all existing
asset, PNG, 8 MiB, inventory and extracted-package byte joins. Register its exact
adapter controls before execution; no acceptance runs during the owner pause.

### Explicit packaged-browser adapter controls

The browser CLI may take exactly five additional operands as an all-or-nothing
tuple: `--product-source-root`, `--product-source-commit`,
`--console-receipt-sha256`, `--console-source-helper` and
`--console-source-helper-sha256`. Require `--packaged-console`; no environment
variable or receipt path may select this mode. The existing `--build` selects
the build root. Pin these operands in caller argv before any later acceptance.
Keep the two-argument development stager and packaging constructor unchanged.

Snapshot the hash-pinned helper into owned disk scratch before invoking it by
absolute path, with no shell, a finite deadline and bounded output. Require its
successful report to join the exact root, commit, build root and receipt digest.
Recheck the receipt's digest before interpreting it and before staging output;
stage source and asset bytes under the existing checks. Record both product
provenance and the independently recorded harness identity in the browser result.
The helper report is not a reusable unpinned admission certificate.

Rust-driven adapter controls must cover: explicit pinned product success from a
different harness; default stale-development refusal; incomplete tuple and mode
without extracted package refusal; wrong helper hash, root, commit or receipt
pin; a receipt changed after helper verification; forged helper report identity;
receipt `source` or environment-only override granting no authority; and changed,
missing, unlisted or symlinked assets plus time-bearing PNG and 8 MiB refusal
after a valid source join. Require rejection before staging output or launching
a browser. Invoke the real production adapter with injected fixture effects,
not a second implementation of its decision. These are harness controls, not
browser acceptance. Preserve the legacy asset controls as well.

Use the browser fixture's existing owned process-group/cgroup boundary for the
helper effect, supplied to the stager as an explicit callback. Do not add a
second Python selector/killpg supervisor. Retain the existing immutable binary
snapshot mechanism where practical. The pinned Rust helper emits a bounded
identity report and fixed diagnostics; its Git output is separately bounded and
not forwarded. Read its owned log with a 64 KiB plus one-byte refusal probe. This
is not a disk-output guarantee for arbitrary caller-selected programs. Bound
the helper phase by 90 seconds and the fixture's original absolute deadline,
whichever is earlier; preserve existing owned-group stop/reaping on failure.
Source controls must check this actual callback wiring and deadline. Existing
native ownership controls remain required; no new process supervisor is admitted.

### Bound legacy adapter materialization

Prospective correction after source review: explicit product mode must read at
most 4 MiB plus one byte of `build.json` before parsing or invoking the helper.
Reject overflow before either effect. The native reader's ceiling alone cannot
bound an earlier Python `read_bytes` allocation. Default development receipt
loading stays unchanged.

Enforce the existing 8 MiB aggregate asset budget before materializing assets.
Require nonnegative integer byte counts (booleans are not counts), canonical
SHA-256 strings and a total within that existing budget. Read each asset with
its declared extent plus one growth byte and retain the exact size/digest/PNG
checks. This tightens representation and allocation admission; it never raises
the budget or changes a product package. Keep it in its own reasoned change.
Add controls for an oversized receipt with no helper invocation, oversized asset
declarations, malformed byte counts, and a physically oversized/sparse asset
whose small declared extent must cause bounded refusal before destination
creation. Preserve the earlier adapter fixture failures and their corrections.

## Measurement and stopping rule

Use seven alternating warm pairs on identical 17-source canonical console
inventories and a deterministic 512-source fixture. Compare existing Python
source hashing only with Rust hashing only, and separately report complete CLI
time including Git. Record CPU/tool versions, source/artifact identities, byte
counts, wall time, peak RSS and cache limitations. Do not compare the full native
Git join to a Python hash loop as equivalent work, or infer a speedup without an
equivalent baseline. Register any later composite benchmark before execution.
Stop optional tuning when controls pass and measurements show no actionable
bottleneck; actual browser and two-hour soak time cannot be optimized away.

### Prospective optimized repeat

X313 used a Rust debug build; retain its actual observations as diagnostic
results, not adoption performance evidence. Python hashlib already uses optimized
native hashing, so repeat with `cargo build --offline --release` before assessing
performance. Keep the same source implementation, host, canonical 17-source and
deterministic 512-source corpora, seven alternating pairs, seven separate complete
CLI observations, controls and measurement boundaries. Change only the Rust build
profile and fresh output path; record new source/ELF identities and raw wall,
CPU/RSS, exit and whole-stop observations. Preserve the debug outputs. This repeat
authorizes no provider, backend, GPU or other parameter search and no acceptance.
