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

The compact JSON envelope is limited to 64 KiB before decoding; nested lists,
identifiers, text, and clock provenance are bounded and validated. Disclosure
states remain explicit. SHA-256 declarations include a representation basis and
are checked for role, syntax, and references here; hash production and
independent verification belong to later custody/verifier work.

## Rationale and limits

Shared run references avoid copying reproducibility context into every record,
while typed digest wrappers keep artifact bytes, manifest bytes, canonical
records, and physical files distinct in Rust APIs. This DTO alone does not
collect, persist, transmit, or verify manifests. Its size choices are initial
wire limits for small provenance records, not measured throughput claims.

## Revisit when

An actual producer requires a larger declared payload or a digest role not
covered by this contract; that change must retain pre-decode bounds and explicit
representation semantics.
