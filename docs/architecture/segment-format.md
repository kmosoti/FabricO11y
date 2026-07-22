# Immutable Segment Format v1

Status: Frozen for the MVP implementation.

## Goals and boundaries

A segment is an immutable archive of newline-delimited canonical evidence envelopes compressed as
one Zstandard frame. The container owns integrity and replay transport. It does not own indexing,
semantic ordering, correction meaning, or random access. Catalog pruning uses the uncompressed
manifest; ordinary search uses maintained SQLite indexes.

All multibyte integers use unsigned big-endian byte order. Implementations must check integer
bounds before allocation.

## Byte layout

```text
offset  size  field
0       8     magic ASCII "FABSEG01"
8       2     format_version, value 1
10      2     flags
12      4     manifest_length
16      8     compressed_payload_length
24      M     UTF-8 canonical JSON manifest
24+M    P     one Zstandard frame
24+M+P  8     trailer magic ASCII "FABEND01"
+8      32    SHA-256(manifest bytes)
+40     32    SHA-256(compressed payload bytes)
```

The fixed prelude is 24 bytes and the fixed trailer is 72 bytes. The payload offset is
`24 + manifest_length`; frame offsets in the manifest are relative to that payload. The exact file
length must be `24 + M + P + 72`; fewer bytes are truncation and additional bytes are trailing
data.

Flags use bit 0 for a referenced dictionary and bit 1 for an uncompressed-content digest. Both
bits are set consistently with the manifest. Bits 2 through 15 are reserved and must be zero.

## Bounds and canonical content

- manifest: at most 65,536 bytes;
- compressed payload: at most 256 MiB;
- uncompressed content: at most 256 MiB;
- records: 1 through 10,000,000;
- frames: exactly one in format v1.

Canonical JSON is UTF-8, no insignificant whitespace, lexicographically ordered object keys, and
one LF byte after every record including the last. Duplicate JSON object keys, blank rows, and
non-v1 evidence envelopes are invalid. Segment identity is `seg-` plus lowercase
`SHA-256(uncompressed canonical JSONL)`. Identical content therefore seals idempotently to the same
path.

## Manifest and pruning metadata

`schemas/fabric.segment.manifest.v1.json` is normative. It records schema-set identity, row and
frame counts, producer-sequence and logical-clock ranges, time ranges, classifications, frame
offsets and sizes, compression configuration, optional dictionary locator, and payload/content
digests. Those fields support catalog recovery and coarse pruning without decompression.
After payload decoding, implementations recompute row count, content identity, classifications,
and every ordering/time bound from the canonical records and require an exact manifest match.
Catalog columns duplicated from the manifest are maintained projections and must agree with both
the canonical manifest and decoded record metadata.

The trailer's manifest digest authenticates the exact uncompressed header bytes. The trailer and
manifest payload digests must agree and authenticate the compressed bytes. After decompression,
the content digest must match both the manifest and segment identifier.

## Dictionary lifecycle

A segment either declares the explicit no-dictionary family (`dictionary: null`, flag 0 clear) or
one locator containing family, monotonically increasing version, and immutable SHA-256 digest
(flag 0 set). Lists, fallback dictionaries, and mutable aliases are forbidden. A resolver verifies
the dictionary bytes before giving them to Zstandard.

`schemas/fabric.dictionary-registry.v1.json` records the training-corpus digest, activation,
deprecation, retention, and rollback set. At most one version per family is active. Activation is a
new registry record, never mutation of dictionary bytes. Deprecated dictionaries remain readable
while any segment references them; `retained_until: null` means indefinite retention. Rollback
selects a retained prior version for new writes and never rewrites existing segment locators.

## Stable failure categories

Parsers check in this order and return one stable category:

1. `truncated` for fewer than 24 prelude bytes or fewer than the declared exact length;
2. `invalid_magic` for bad prelude or trailer magic;
3. `unsupported_version` or `unsupported_flags`;
4. `header_too_large`, `payload_too_large`, `content_too_large`, or `row_limit_exceeded` before allocation;
5. `trailing_data` for bytes after the exact trailer;
6. `manifest_checksum_mismatch`, then `manifest_invalid`;
7. `payload_checksum_mismatch`;
8. `missing_dictionary` or `dictionary_digest_mismatch`;
9. `decompression_failed` for malformed Zstandard data or a payload that is not exactly one frame;
10. `content_checksum_mismatch`;
11. `row_invalid` for malformed or semantically invalid JSONL.

No category permits partial replay. Catalog/segment disagreement is checked above this container
boundary and likewise fails without silently omitting evidence.
