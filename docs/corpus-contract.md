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
