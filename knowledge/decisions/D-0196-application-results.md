# Bounded application result identity

The shared `fabric-evidence` crate exposes this contract to the SDK and retains
the existing `fabric-core` paths as re-exports.

Application reporting needs to preserve the identity of the exact source file
and of its normalized JSON content. A trailing newline or Unicode escape can
change source bytes without changing the decoded application payload. The
contract uses distinct SHA-256 roles for these byte sequences and checks their
semantic relationship when the source artifact is supplied. It does not equate
either digest with the enclosing custody observation or frame.

The initial profile is deliberately narrow: bounded JSON objects with exact
integers, sorted UTF-8 keys and compact encoding. It rejects duplicate decoded
keys, fractional and exponent numbers, negative zero and out-of-range integers.
This supports the initial fixture without inventing floating point semantics.
Applications needing a wider numeric domain require a new reviewed profile.

Report and optional parent IDs are scoped to origin and producer. These are
labels, not authenticated authority. A future adapter must bind them to its
verified identity and authorize parent retrieval. The pure contract does not
persist, deduplicate or deliver reports. The batch adapter validates these
labels against its configured producer and leased origin and carries the
bounded envelope in a v3 JSON log. Deduplication is confined to one outbox.

The outbox records the producer's completion claim and a local wall-clock sample
inside the same transaction that stages the frame and sequence. This observation
precedes transaction commit and is not a measurement of fsync, Spindle custody,
or visibility. Pending and confirmed rows retain it across replay; migrated v1
rows have unknown timing. No original artifact bytes are retained. Parent
authorization and receiver visibility remain separate contracts.
