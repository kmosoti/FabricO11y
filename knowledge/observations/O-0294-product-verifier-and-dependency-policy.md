---
id: O-0294
date: 2026-11-27
added_by: agent
human_ratified: false
---

# Product verifier and dependency-policy evidence

This is a source-bound observation for [#294](https://github.com/kmosoti/FabricO11y/issues/294).
The local qualification used commit `794e1f92b5295d45bb574813f30f4f60ecab748a`
with tree `47c408761ff3bb13cf358c220814801dc0ffb39e`. It supplements the earlier
proof inventory in [O-0300](O-0300-proof-inventory-and-trust-boundaries.md) and
the product/verifier separation recorded in
[D-0006](../decisions/D-0006-verifier-cache-provenance.md).

## Current-source execution

The local run used the pinned `rust-toolchain.toml` toolchain: `rustc 1.100.0`
(commit `0ef1bc386059596dada8fb65b50425aab2ea637a`) and Cargo `1.100.0`
(commit `3acc79c0c`). Both offline locked metadata commands succeeded on this
source:

- The root workspace resolved 353 packages and 17 workspace members. Its
  `Cargo.lock` SHA-256 was
  `89b530a8db566e76784c4607091e364abc17fe9af6da5a022eb1622a05cfdaa1`; the
  metadata output SHA-256 was
  `ff20063611a066d89a78ad6b0401c98f7b063a3682a272b3b4e49a4dee58aa5a`.
- The standalone fuzz workspace resolved 151 packages and 1 workspace member.
  Its `fuzz/Cargo.lock` SHA-256 was
  `a7fdd55490f53cff1fd10e3ac2d06b7ffa21f0bc0535a5463bd0f5067cd506a2`; the
  metadata output SHA-256 was
  `10787f189010bdc523747b20ce878412c81a7bb2735db06cd22a04458205165b`.

Workspace formatting, architecture-boundary checks, and the documentation
inventory passed on this source. `cargo xtask prove selftest` passed its positive
controls and rejected both false-property fixtures, the projection-frontier
mutant, and the missing-checker path. The host-built `xtask` executable
SHA-256 was
`a251e804b571f85b6caa80ef79bae43e4df13ab589ec6131d7bea60e09d94995`; its
implementation source `xtask/src/proof.rs` was
`ea5c2768d80e690f0b4d90a0efbc83dc832178ef38ff7008671fec92ac3ad807`.

The captured local transcript SHA-256 is
`acd3de87e0de47ff561529a127f4ec209eb624c604507a555fe95216f9dca7af`. It
records the exact versions, binary digests, fixture digests, metadata artifact
digests, and command results for this run. The transcript predates this O-0294
record; it does not claim to validate this documentation change.

## Tool pins and observed identities

### Product Rust and Cargo

- Pin: `rust-toolchain.toml` selects Rust `1.100.0`, Edition 2024.
- Observed: `rustc 1.100.0`, commit
  `0ef1bc386059596dada8fb65b50425aab2ea637a`; Cargo `1.100.0`, commit
  `3acc79c0c`.
- Qualified here: formatting, locked metadata resolution, and the host-built
  `xtask` checks above.
- No root or crate manifest declares `rust-version`; the pin is not a blanket
  compatibility promise.
- Not established: a broader MSRV, compatibility with other Rust patch
  releases, or compilation of every product crate on this source.

### Verus and Z3

- Verus pin: `0.2026.10.26.d17fbd0`; installer archive SHA-256
  `361674d05f0c1b2c9c3f4d5bc56a8fb2ab9241ed0cceca27780668c292bb4940`.
- Observed Verus: version matched; it reports internal Rust `1.99.1`; binary
  SHA-256
  `1208c995c8e5244577693eba311d34bc2050c4acf984e10c4c7baf2003abff14`.
  It ran the positive/false fixtures and frontier mutant directly. This
  verifier compiler is separate from product Rust.
- Z3 pin: Verus-bundled `4.16.0`. Observed `Z3 version 4.16.0 - 64 bit`,
  binary SHA-256
  `e583c4186a45e72411fa2cb2048401eed03f0f8e5f24694676a8f6271a50b765`.
  The qualification recorded its identity; it did not run the standalone SMT
  proof command.

### Lean and Lake

- Lean pin: `4.35.0`; installer archive SHA-256
  `7289597a44543246c9227b8cae501767e095ce17f6d448b95fc4a8dc97268f6d`.
- Observed Lean: `4.35.0`, commit
  `eb6cafb7290e8bd5f2937522d8d804f58f02bafc`, binary SHA-256
  `bf8d54e4714cc4b03d3f6bb34c83b7202b87e49c8bfcbff6895c085bb90ceb38`.
  Lean ran the positive/false fixtures.
- Observed Lake: `5.0.0-src+eb6cafb`, binary SHA-256
  `8ba83c98f76562d03b8dbc33f1057151ba6cbb121679dde76b2260ccc90495e1`.
  Lake was identity-probed but was not invoked by this self-test.

### Charon, Aeneas, and extraction environment

- Installer pins: Charon commit
  `7b462d9159db2ed3015162352183906a30bfafc6`, version `0.1.304`, extraction
  Rust `nightly-2026-09-17`; Aeneas commit
  `ab34bd28458a08c150b7cbf1873e59af74588138`.
- O-0300 records earlier, separate local probes at its
  `7de9e1918802f51ca108b1e9fd54005236982390` baseline: Charon binary SHA-256
  `655e8bfd8b7726067ae5e82f1248007b0e5fbafb89b22afa93aa24eb625627ce` and
  Aeneas binary SHA-256
  `75ec89d46d77777a422538907f43752dac498bf9c29b6fb02efedd9de492b145`. These
  are historical probe identities, not executions on `794e1f92`.
- The extraction lock pins OPAM `2.6.1`, OCaml `5.3.0`, and the package set;
  `verification/aeneas.opam.lock` SHA-256 is
  `07f44704489399e345b3aff6411850f0a1e1b1c2e9cce795bdc967432207a828`. No
  extraction or OPAM environment was run on `794e1f92`.
- The installed `cargo-verus` binary SHA-256 was
  `534c879934d761baf62301fb5d6eb1e9b3aeec3d1cc72f15da03ef4ae0df95bc`. It was
  not invoked by `prove selftest`; its CLI supports `--help`, while
  `--version` is unsupported and yields no version result here.

## Self-test fixtures

The fixture bytes below are the exact strings written by `xtask/src/proof.rs`
(SHA-256 `ea5c2768d80e690f0b4d90a0efbc83dc832178ef38ff7008671fec92ac3ad807`);
their hashes do not include a trailing newline.

| Fixture | SHA-256 | Observed result |
| --- | --- | --- |
| Verus positive `valid_transition` | `301a22a666e82fc151814768225365fe216fcbded923d29c91fced3e7bd4ec12` | Accepted |
| Verus false `invalid_transition` | `6810ae60aa3402c898513070551576a97da721cf2ce6af904dd92aec1548cb03` | Rejected, exit status 1 |
| Lean positive `True` | `c12b4508228c6c6c23a614c28241134daa0e65263542143fd9b60e204a300147` | Accepted |
| Lean false `False` | `1901ee281e2ce7466c4d7aaf8ade664f0eaf683dd999e1d2e1d55bd576d93a70` | Rejected, exit status 1 |
| Projection-frontier mutant | Mutates the flush-success gate in `admission-kernel/src/telemetry.rs`; source SHA-256 `d52a4ffb7343d24241d338067d5de924c44ba38fa1a38abdd77d6bb4e7b1cc8c`; mutant SHA-256 `9e0f7226aef3e36156edbb55307c99f149e182f8870ba2f8e09edf4bca1d073d` | Rejected by a Verus postcondition |

The root `Cargo.lock` pins `vstd` as `0.0.0-2026-10-25-0158` with checksum
`e738decec9a809c06589281c48d96929e007079707d4e03018734daa4b20967f`, but the
standalone Verus fixture imports `vstd` directly and this self-test does not
establish which package it resolves. It does not run `cargo xtask prove all`, the
Aeneas extraction, Charon/Aeneas, a product-crate build, fuzz execution, or the
full model suite. Passing controls establish only the listed positive and
negative cases.

## Dependency-policy coverage and unsupported claims

In the captured local environment, `cargo-deny`, `cargo-audit`, and `cargo-vet`
were not installed; no `deny.toml`, `audit.toml`, or `vet.toml` policy files were
found in this checkout. The two locked metadata checks establish that the root
and fuzz dependency graphs resolve from their checked-in lockfiles with Cargo
`1.100.0`. They do not audit advisories, licenses, unused dependencies, or
package publish age. No minimum-publish-age gate, unused-dependency gate,
exception policy, or CI pin for such a tool is evidenced here.

The only product compiler qualified in this snapshot is the pinned Rust
`1.100.0`. No compatibility claim is made for other Rust patch releases or
combinations of product Rust with verifier compiler internals. Kani, Loom,
other mutation frameworks, and the dependency-policy tools above are not
qualified by these results.

## Earlier hosted result and source boundary

O-0300 records GitHub Actions run `45942052238` as successful for source commit
`7de9e1918802f51ca108b1e9fd54005236982390`, including `verify` and
`native-acceptance`. That hosted result is evidence for that exact baseline
only. O-0300's source comparison records that the root lockfile has since
changed from baseline SHA-256
`80981a1dcf51ec4b171bfd843089822660c1e9db1a71f203fe590b57cdbe4c78` to
`89b530a8db566e76784c4607091e364abc17fe9af6da5a022eb1622a05cfdaa1`. On
`794e1f92`, the evidence consists of the local checks listed above; no hosted
result for `794e1f92` is asserted.
