# D-0236 · Bounded run and attempt manifests

- Date: 2026-11-27
- Status: working contract
- Added by: agent
- Human ratified: false

## Decision

Keep run context and attempt provenance in versioned `fabric-core` Sans-I/O
manifests. A run manifest owns shared code, tree, configuration, input,
dataset/cohort, evaluator, toolchain, capture-policy, and artifact references.
Attempt manifests point to the run by immutable ID and digest and may state
explicit input, evaluator, toolchain, or capture-policy overrides. Observations
point to an attempt by immutable ID and digest and carry their own record
identity, digest roles, and independently clock-qualified times.

The compact JSON envelope is limited to 64 KiB before decoding and uses a
64 KiB capped JSON writer on encode. Nested lists, identifiers, text, and clock
provenance are bounded. Disclosure states remain explicit. Run and attempt
digests use the closed typed `fabric.manifest.canonical-json.v1` payload and
exclude their own `digest` member; validation recomputes and compares those
hashes. Exact artifact, original-record, canonical-record, and physical-file
bytes have separate SHA-256 wrappers and exact-byte verification methods.

The profile intentionally does not canonicalize arbitrary JSON values. Generic
maps can hide duplicate-key ambiguity when parsed, and float spelling requires
additional cross-codec rules. The existing v3 production wire codec serializes
typed `ObservationV3` with `serde_json::to_vec`; Spindle custody retains the
submitted raw payload bytes. This contract leaves those payload bytes unchanged
and verifies canonical-record claims over exactly those bytes. Its separate
manifest profile uses a closed integer-and-string-only type schema and has a
Python-generated known-answer byte/digest vector. An exact-byte fixture
`{"label":"caf\u00e9"}\n` hashes to
`426d8f9948cd8aa9aede8235c65e8f5fd79eb048055e962afbea5c2b5ca1b64d`, while
`{"label":"café"}` hashes to
`2d7fba14f0a7cffed454bc268d791c0f32d11f62946998a323087ccf5df29075`.
Consequently, a source file's trailing newline and Unicode escape spelling stay
part of its original-artifact digest, independently of record and stored-file
bytes.

## Rationale and limits

Shared run references avoid copying reproducibility context into every record,
while typed digest wrappers keep shared artifact content, source artifact bytes,
manifest bytes, canonical records, and physical files distinct in Rust APIs.
Input overrides permit at most 32 unique locators and one each of evaluator,
toolchain, and capture-policy overrides. Fixed run fields must carry their
matching artifact kind. If an attempt has any `Input` override entries, all of
them replace the run's complete input set. They are not appended or matched to
run inputs by locator. An attempt without input entries inherits the run's
explicit input disclosure. Empty replacement is unsupported; `available([])`
at the run level explicitly describes an empty set. This avoids treating a
missing override as either clear or partial replacement.

The manifest digest profile does not define canonical bytes for #196's future
application result schema. That result needs its own closed, versioned encoding
contract; until it exists, canonical-record digests cover the exact accepted
payload bytes and original artifacts retain their separate exact-byte hashes.

This DTO alone does not collect, persist, transmit, or verify manifests. Its
size choices are initial wire limits for small provenance records, not measured
throughput claims.

## Revisit when

An actual producer requires a larger declared payload or a digest role not
covered by this contract; that change must retain pre-decode bounds and explicit
representation semantics.
