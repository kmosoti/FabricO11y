# Existing proof inventory and trust boundaries

This inventory records the proof and checker boundary at the `develop` source
revision `7de9e1918802f51ca108b1e9fd54005236982390`, for M1 issue
[#300](https://github.com/kmosoti/FabricO11y/issues/300). It is a baseline for
the broader runtime-coverage work in [#235](https://github.com/kmosoti/FabricO11y/issues/235),
not a claim that #235's acceptance criteria are complete. The related
[#248](https://github.com/kmosoti/FabricO11y/issues/248) calls for preserving
these kernels and negative controls while separately qualifying compatibility,
new tools, and protected-specification review.

## Baseline evidence

For this exact commit, GitHub Actions run
[45942052238](https://github.com/kmosoti/FabricO11y/actions/runs/45942052238)
reported both `verify` (2026-11-27 04:59–05:18 UTC) and
`native-acceptance` (04:59–05:26 UTC) as successful. The checked-in workflow
shows `native-acceptance` invoking `cargo xtask check --group native`, engine
custody/recovery regressions, and fuzz smoke. The native group runs coverage,
ignored collection and distributed program tests, architecture checks,
`prove all`, `prove selftest`, and corpus replay. The check result is evidence
for those commands at that commit; it does not establish broader release
qualification.

The product toolchain is Rust 1.100.0, Edition 2024 (`rust-toolchain.toml`).
The tool installer and `xtask` source pin/check the following proof toolchain:

| Tool or input | Pinned identity in source | Observed local checker identity |
| --- | --- | --- |
| Verus | `0.2026.10.26.d17fbd0`; download SHA-256 `361674d05f0c1b2c9c3f4d5bc56a8fb2ab9241ed0cceca27780668c292bb4940`; Verus reports its internal Rust `1.99.1` toolchain | version matched; binary SHA-256 `1208c995c8e5244577693eba311d34bc2050c4acf984e10c4c7baf2003abff14` |
| `vstd` | Cargo.lock `0.0.0-2026-10-25-0158`, checksum `e738decec9a809c06589281c48d96929e007079707d4e03018734daa4b20967f` | The self-test does not establish which `vstd` supplies its standalone Verus fixture; see below |
| Z3 | `4.16.0`, bundled with Verus | version matched; binary SHA-256 `e583c4186a45e72411fa2cb2048401eed03f0f8e5f24694676a8f6271a50b765` |
| Lean / Lake | Lean `4.35.0`; archive SHA-256 `7289597a44543246c9227b8cae501767e095ce17f6d448b95fc4a8dc97268f6d` | Lean `4.35.0`, Lake `5.0.0-src`; Lean binary SHA-256 `bf8d54e4714cc4b03d3f6bb34c83b7202b87e49c8bfcbff6895c085bb90ceb38` |
| Charon | commit `7b462d9159db2ed3015162352183906a30bfafc6`; version `0.1.304`; compiler `nightly-2026-09-17` | all three version checks matched; binary SHA-256 `655e8bfd8b7726067ae5e82f1248007b0e5fbafb89b22afa93aa24eb625627ce` |
| Aeneas | commit `ab34bd28458a08c150b7cbf1873e59af74588138` | source revision matched; binary SHA-256 `75ec89d46d77777a422538907f43752dac498bf9c29b6fb02efedd9de492b145` |
| Extraction environment | OPAM `2.6.1`, download SHA-256 `b533d393a6967150d6ef41cb3d71e147e93ecbabeb59e921aa412c0ae79ebe3d`; `verification/aeneas.opam.lock` freezes OCaml `5.3.0` and the transitive package set | source/lock pins inspected; no extraction run in this inventory task |

The local checker versions and binary digests above identify the tools observed
in this environment; except for Verus and Lean in the self-test log, the local
version/hash probes were separate from a suite run. None are presented as the
hosted runner's binary digests. The installer pins/checks extraction Rust
`nightly-2026-09-17`, Verus-compatible Rust `1.99.1`, and cargo-fuzz `0.13.2`.
Product Rust remains the independent `1.100.0` pin.

The following checked-in source digests are SHA-256 values from the baseline
revision. They identify both the specifications and the harnesses that select
and audit them:

| Source | SHA-256 |
| --- | --- |
| `admission-kernel/src/lib.rs` | `03bda9c41140537b88bb2067e3e276cd6c58933894157393ab7e4a225e49d63f` |
| `admission-kernel/src/delivery.rs` | `28666e869988366268cfc812e0e00bf403c85eb740d33bddde0b0a02aa2b28d8` |
| `admission-kernel/src/telemetry.rs` | `d52a4ffb7343d24241d338067d5de924c44ba38fa1a38abdd77d6bb4e7b1cc8c` |
| `proofs/ExtractedAdmission.lean` | `cef2550614599ccad27d73ba09bb3dc4650f0ca311889691086e3a9b5c81a9a1` |
| `proofs/Admission.lean` | `a00ece09237ee14bf56dffc844b0106c542ee05eb5baa3c1a854196009e7d59b` |
| `proofs/Collection.lean` | `cd443629bee0ab97035dc2a13de93c00f68dc03bc82029e93190c2a7ed074e37` |
| `proofs/Distributed.lean` | `513bffe12f61c76460708ab108c28a9ffb9867252f3aca4542d732f256a34f4f` |
| `proofs/Custody.lean` | `8ff6f089ce317903fa5d68f04ac822436ea1e33881e5dc5dc269584e8ab87ca4` |
| `proofs/Telemetry.lean` | `a6b7309fd454779ce88d6d1ae96fc4294fdf0722c1639b9e4bd2561c29bcda32` |
| `proofs/TelemetrySmt.smt2` | `092f3c06d4561b712774915716026e68b4730e5d06bcf621fa1c3518b9b6c08e` |
| `proofs/admission.rs` | `c523b56a4750a792107d2012abb56d924945db054b5e64ed322dabff35d38eba` |
| `xtask/src/proof.rs` | `ea5c2768d80e690f0b4d90a0efbc83dc832178ef38ff7008671fec92ac3ad807` |
| `xtask/src/extraction.rs` | `3ac80157a80439c81840b6447ca9b954ab2b6f5807d72f1eb3d1a0d99bba5373` |
| `verification/aeneas.opam.lock` | `07f44704489399e345b3aff6411850f0a1e1b1c2e9cce795bdc967432207a828` |
| `Cargo.lock` | `80981a1dcf51ec4b171bfd843089822660c1e9db1a71f203fe590b57cdbe4c78` |
| `rust-toolchain.toml` | `2005a6440b4cf56de43085410f26315d73dece6d34a6d32920669f65f33062c4` |
| `.github/scripts/install-verification-tools.sh` | `e20ba9a0ec8e97cee490d08aac633d5d0ea3d5170671413744ad37f74a6260b6` |

This list is an inventory snapshot, not an enforcement manifest: the extractor
does not require these values as expected hashes. It records source and property
digests for a run and checks that those files did not change between its
before/after observations. It also prints tool binary digests but accepts tools
by their reported version or source revision, not by comparison to a pinned
binary digest. The installer separately verifies release archive hashes and
checks its managed Aeneas build identity against its local stamp.

## Prospective current-base source comparison

On prospective develop `21eef26cee1d79eb6b30bb7dfa9ff8dbb7d411bd`
(tree `d1568cee5dc4de4ff5236e0d90ed662f3231320e`), a read-only SHA-256
comparison found 16 of the 17 historical source entries above still match
their `7de9e1918802f51ca108b1e9fd54005236982390` baseline. The sole
difference is `Cargo.lock`: its historical baseline SHA-256 is
`80981a1dcf51ec4b171bfd843089822660c1e9db1a71f203fe590b57cdbe4c78`,
while its current SHA-256 is
`89b530a8db566e76784c4607091e364abc17fe9af6da5a022eb1622a05cfdaa1`.
This source audit does not run or revalidate any checker; the proof results
above remain bound to their recorded baseline/candidate commits, not this
prospective source revision.

## Suite inventory

| Suite and selector | Obligation actually checked | Refinement boundary and limits | False / missing-checker control |
| --- | --- | --- | --- |
| Verus executable kernel: `cargo xtask prove verus`; `admission-kernel/src/lib.rs`, `delivery.rs`, `telemetry.rs` | Capacity/byte arithmetic; private slot identity, capacity, encoded length and Free/Reserved/Ready/Leased transitions; FIFO ticket bounds/overflow; delivery frontiers; monotonic watermarks; contiguous origin-qualified projection frontier after successful flush; elastic memory lease arithmetic; unflushed ring headroom; disk-eviction and RSS-threshold predicates. Cargo Verus verifies the crate with `--no-cheating`, locked dependencies and two solver threads. | This checks specified kernel functions, not all adapter schedules or process behavior. `vstd` specifications and the Verus compiler/solver stack are trusted. SDK synchronization, buffer ownership, lease cleanup and I/O remain in Rust adapter tests. The full kernel command uses Cargo and its lockfile; this does not imply that standalone self-test fixtures use that locked crate. | `prove selftest` invokes Verus directly on synthesized standalone fixture files: it proves a positive fixture, rejects `ensures false`, and rejects a projection-frontier mutant that removes the flush-success gate. The fixture imports `vstd::prelude::*`, but this run does not record or establish which support crate Verus resolved. The self-test also confirms a nonexistent checker cannot run. Delivery has a separate skipped-receipt mutant in `prove collection`. |
| Charon → Aeneas → Lean extraction: `cargo xtask prove aeneas`; source selector above; generated `AdmissionKernel.lean` plus `proofs/ExtractedAdmission.lean` | Extracts the same three Rust source files, translates them, rejects opaque/incomplete generated definitions, and checks 25 named extracted contracts for admission, slot/FIFO transitions, watermarks/frontiers and telemetry predicates. The checker verifies source and property hashes are unchanged during this run and audits each named theorem's axioms. | This is the direct source-to-model path, but the theorem file supplies the properties over generated definitions; it does not prove the Rust adapter, OS/storage implementation, compiler/extractor correctness, or runtime refinement. No Lean FFI is used by the product. Allowed Lean axioms are `propext`, `Classical.choice`, and `Quot.sound` (or none). | The self-test contains positive and false Lean fixtures. Extraction checks tool-reported expected versions/revisions, source stability during execution, opaque `External` output, `axiom`, `sorry`, `sorryAx`, and the theorem audit. It does not compare tool binary digests or the observed kernel/property digests to pinned expected SHA-256 values. |
| Standalone historical Lean admission: `cargo xtask prove lean`; `proofs/Admission.lean` and `proofs/admission.rs` | Two natural-number model theorems for preserving capacity bounds and rejecting overflow. Axiom audit allows `propext` and `Quot.sound`. | Historical independent model/candidate only; not extraction coverage and not evidence of whole-SDK behavior. | `prove selftest` accepts `True` and rejects `False`; no claim that this is an executable-Rust mutant. |
| Collection model: `cargo xtask prove collection`; `proofs/Collection.lean` (13 audited theorems) | Capacity/retention arithmetic and a durable transaction model: duplicate/conflict/abort behavior, jointly published commit, and receipt implies retention. | Model assumes atomic storage; not a proof of SQLite/other engine or the complete runtime path. | Skipped-receipt mutation of `admission-kernel/src/delivery.rs` must fail the Verus postcondition. |
| Distributed custody model: `cargo xtask prove distributed`; `proofs/Distributed.lean` (18 audited theorems) | Distinguishes local Spindle seal from remote Fabric seal; publication, conflict refusal, duplicate idempotence, origin isolation, contiguous frontier, and reclamation only after remote custody. | Atomic stable storage and codec refinement are assumptions; not proof of database transaction or wire codec implementation. | A conflict-overwrite mutant must violate the audited Lean theorem. |
| Two-store custody model: `cargo xtask prove custody`; `proofs/Custody.lean` (30 audited theorems) | Exact byte-list/source/retry identity bindings, fresh canonical IDs, uncertainty retention, full readback before visible publication, contiguous source prefix, allocation bounds, shadow retention, receipt-scoped reclamation, lease expiry/retirement and cancellation credit until worker cleanup. | Atomic durable operations, receipt provenance, identity encoding, byte-faithful runtime readback, and cleanup/worker termination correspondence are assumptions. Natural counters do not prove Rust overflow behavior; filesystem, SQL engine, crypto, synchronization and process memory are outside the model. | Conflicting-reservation and publication-without-readback mutants must be rejected. |
| Bit-vector telemetry SMT: `cargo xtask prove smt`; `proofs/TelemetrySmt.smt2` | Four Z3 `QF_BV` obligations: ZigZag bijection, modular Gorilla timestamp delta-of-delta round trip, IEEE-754 XOR zero trimming, and watermark monotonicity. Expected results are all `unsat`. | Bit-vector formulas only; they are not a proof of Rust serializer implementation correspondence or persistent filesystem durability. | Mutates `bvsub t1 t0` into unsigned saturating subtraction: return the modular difference when `t1 ≥ t0`, otherwise zero. Z3 must produce a satisfying counterexample (`sat`). |
| Standalone telemetry Lean: `cargo xtask prove telemetry`; `proofs/Telemetry.lean` (14 named theorems in current source) | Gorilla stream round trip, Euclidean 5-minute rollup alignment/idempotence/monotonicity, failed-flush and trace watermark properties, and elastic budget/lease-return properties. | Unbounded Lean integers model timestamps only where the aligned result fits `i64`; saturating lower-bound cases are documented. It models arithmetic, not `/proc` or cgroup sampling, fsync, or atomic rename. | The checker audits theorem axioms and rejects the premature-watermark-advance mutant; targeted named telemetry mutants are required by the checker. |

`proofs/README.md` also describes model assumptions and the same distinction
between Sans-I/O predicates and their I/O callers. In particular, a theorem that
`rss_shedding_decision(rss, limit)` is equivalent to `rss > limit`, or that the
janitor predicate is equivalent to usage exceeding quota, does not establish
that Linux measurements are accurate, that the daemon reaches the branch under
load, or that filesystem operations survive power loss.

## Runtime trigger evidence still separate from proofs

Source inspection found relevant executable fixtures: daemon tests inject a
failed OLAP flush and check that the watermark stays put until retry succeeds;
janitor tests such as `compacts_downsamples_and_evicts_over_quota` exercise
quota-triggered eviction. These are useful runtime-path evidence, but they do
not by themselves establish power-loss durability of the filesystem, all
post-flush failure points, or broad production behavior. The self-preservation
guard reads `/proc/self/statm` and cgroup pressure and the daemon uses the
shedding flag to alter collection; its present direct unit-test module covers a
read-only `/tmp` FD-scan case, not a controlled above-threshold RSS trigger and
end-to-end shedding observation.

Accordingly, #235's requested native failure-coverage acceptance remains a
separate task: require explicit evidence that post-flush durability failures,
over-quota eviction, and active RSS shedding are triggered and observed through
the relevant runtime paths. This inventory does not assert those gaps are
closed, does not claim #235 acceptance, and makes no new proof or policy claim.

## Local verification performed for this inventory

On Rust `1.100.0`, `cargo fmt --all -- --check` passed. The targeted command
`cargo xtask prove selftest` passed under the workspace's heavy-job disk gate:
the false Verus fixture and false Lean fixture were rejected, the
partial-publication Verus mutant was rejected for its postcondition, and the
missing-checker control refused to run. The saved log includes Verus and Lean
version output and their binary SHA-256 values from the same environment used
for that command. It records outcomes, not the resolved `vstd` for the direct
Verus fixture. This is a checker-control result, not a rerun of all proof suites
or an extraction run. No verification dependencies, host configuration,
product behavior, or proof specifications were changed.

## Required native runtime controls

The coverage manifest selects exact library fixtures for partial-flush and
deferred-watermark protection and for over-quota partition eviction. These
checks are part of native acceptance. The real RSS guard fixture has a
separate confined Linux profile with one owned process and exact cgroup
limits. Its optional-history branch test does not cover daemon scheduling
with concurrent cgroup pressure. The observed checks above remain evidence
for their recorded source and tools.
