# Research corpus record contract

`fabric-corpus` is a transport-free, Sans-I/O contract. A logical object ID names
an object; a SHA-256 digest identifies one immutable revision body. Corrections,
retractions, and supersessions are represented by new bodies, with prior revision
digests as parents. Admission requires every parent to be already admitted and
belong to the same object. This admits concurrent heads and makes revision
ancestry acyclic. Out-of-order intake must stage missing parents externally.
Semantic relations are attributed claims between revisions and can form cycles;
no entailment is inferred. Their SHA-256 identity covers the complete canonical
assertion, including attribution and extensions. Inserting an identical relation
is idempotent and reports a duplicate; a distinct assertion with the same digest
is rejected. Attestations are separate append-only postcommit claims; they are
not deduplicated and do not change a revision digest. The in-memory corpus is not a durable ledger,
authorization system, receipt verifier, or active-head query.

Profile 1 hashes exactly the bytes returned by `RevisionBody::canonical_bytes`.
The byte layout is `fabric-corpus/revision`, NUL, byte `01`, then object ID,
kind, text, parents, sources, and extensions in that order. Strings are UTF-8
preceded by an unsigned 64-bit big-endian byte length; raw digests are 32 bytes.
Collections have a 64-bit big-endian count. Parents sort by digest bytes;
extension map keys sort by UTF-8 bytes. Kind tags 0–7 follow the declaration
order; custom kind has two strings (namespace, name). Captured source tag 0 is
followed by capture ID, artifact digest, locator, parser; external-only tag 1
is followed by locator and parser. Extension value tags are null 0, false 1,
true 2, text 3, array 4, and object 5. Arrays preserve order and objects encode
key followed by value. No Unicode normalization or numeric conversion occurs;
exact numbers can be represented as strings. Profile bytes are not JSON or JCS.
No serialized import format is specified here: adapters must reject duplicate
keys before constructing maps rather than silently discard input.

Each canonical record is limited to 1 MiB, 4096 aggregate collection items,
and extension nesting depth 32. These limits apply during encoding; callers
must independently bound decoding, staging and total retained corpus size.
Locators must have a nonempty URI-style scheme and remainder and no whitespace;
this is only a shape check. Captured claims include a capture ID and raw artifact
digest, while external-only claims explicitly lack those fields. Neither form
proves availability, custody, parser correctness, or a qualified receipt. Those
require authorized custody adapters and the future durable-result integration.
Unknown extensions are preserved as inert data. The contract does not execute or
assign authority to them.

Relation profile 1 begins with `fabric-corpus/relation`, NUL, byte `01`,
then subject digest, predicate, object digest, attributed-to string and extension
map. Predicate tags 0–5 are supports, refines, supersedes, falsifies, waives,
and custom; the custom tag is followed by namespace and name strings. All
other field encodings and limits are the same as the revision profile. The
digest is SHA-256 over exactly those bytes. Both canonical bytes and digest are
available from `Relation`, so other implementations can reproduce the identity.

## Portable frozen views

The version 1 portable view encodes explicit as-of membership, including
missing selected revisions and relations, and the frozen attributed support
groups. It does not alter the revision or relation profiles above. Export
returns bytes and their SHA-256 digest. Import requires the expected digest
from the caller, checks all included record identities and canonical forms,
rejects reordered or duplicate membership, and rebuilds the same bounded view.
The digest establishes integrity only relative to the supplied expected value;
it does not authenticate its source or prove original corpus admission.

The byte layout is `fabric-corpus/view`, NUL, byte `01`, then an unsigned
64-bit big-endian revision count. Each revision is its 32-byte digest, a
presence byte (0 missing, 1 present), and, if present, a 64-bit byte length
followed by the canonical revision bytes. Two availability bytes follow:
archive then current. Tags are exact 0, aggregate 1, reference-only 2 and
missing 3. These describe declared evidence availability and do not verify
artifact bytes or access. Every selected revision has both claims, including a
missing revision. Revisions sort by digest bytes.

A 64-bit relation count follows, then sorted relation digest, presence byte,
and, when present, length and canonical relation bytes. A 64-bit group count
follows. Each group contains its conclusion digest, a length-prefixed UTF-8
attribution, a 64-bit premise count and premise/relation digest pairs sorted
by premise digest. Groups sort by conclusion, attribution, then their ordered
premise maps. No timestamps or order of arrival are implied. Import bounds the
whole envelope to 5 MiB, retains the existing view limits, and rejects trailing
bytes. It does not import attestations, perform storage reads or enforce
retention; a partial view need not include all revision parents.
