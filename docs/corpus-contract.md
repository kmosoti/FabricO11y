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

## Bounded lexical rebuild

`fabric-corpus::lexical` derives directed adjacent-token occurrence counts from
the text of present revisions in an explicit as-of view. A token is a maximal
run of ASCII letters and digits, converted to ASCII lowercase; every other byte
is a delimiter. Adjacent pairs never span revisions. Results are sorted by pair
and retain sorted contributing revision digests, selected digests and missing
selections. The projection is rebuildable and makes no authorization, custody,
search relevance or incremental-index claim.

The caller can lower each fixed hard cap: 4 MiB scanned text, 262144 tokens,
128 bytes per token, 32768 distinct pairs, 65536 pair-to-revision entries and
4 MiB logical output. Logical output counts pair token bytes and an eight-byte
count, plus 32 bytes per selected digest, missing digest and provenance entry;
it is not a wire format or an allocator measurement. Costs also report scanned
bytes, tokens and pair occurrences. Limit failure returns no partial result.

## Frozen citation context

`fabric-corpus::context` assembles caller-selected citations over an immutable
as-of view. Before verification it bounds the request to 128 citations, 8 MiB
of total supplied artifact bytes and 1 MiB of total requested passage bytes.
It verifies supplied captured bytes and passage hashes against the source claim
in the selected revision. An unselected revision fails; a selected missing
revision, omitted captured bytes and an external-only source remain distinct
from verified supplied bytes. Invalid supplied bytes reject the context. The
view retains selected dissent and relation claims. Neither a recorded claim nor
a digest establishes authorization, custody, availability, or truth.

## Explicit selected-head reconciliation

`fabric-corpus::reconcile` analyzes same-object revision ancestry in a frozen
as-of selection. It reports maximal present selected revisions, all selected
missing revisions and parent digests omitted from the selection. A missing
selection has unknown object identity, so any such selection makes the analysis
incomplete. Reconciliation refuses incomplete selections and requires a new
body whose parents are exactly the two or more considered heads. It rechecks
selected bodies against the supplied corpus before ordinary append admission.
The operation retains all prior revision, relation and dissent claims.

Analysis is bounded by the as-of view limits, 4096 ancestry links, 256 distinct
unselected parent digests and a 1024-byte object ID. These are limits on a
selected snapshot, not evidence that every concurrent global head was selected.
This pure operation does not perform an authoritative global head query or an
atomic conditional write; callers must provide those boundaries where needed.

## Selected dependency differential

`fabric-corpus::differential` imports two portable views after checking each
against its caller-supplied expected digest. The caller binds those digests to
the intended snapshots; a matching digest alone establishes no authority.
Explicit revision and relation digests and group selectors define the scope.
Results retain both snapshot digests and distinguish an omitted selection from
a selected missing record and a present record, including revision availability
claims. Group selectors use conclusion digest and attribution and report the
selected matching group claims and their frozen projections.

A known change to a selected dependency is stale. If no known change exists
but any selected dependency is omitted, missing or an unresolved group,
applicability is unknown; otherwise it is unchanged. Availability labels are
compared as recorded claims and do not prove present availability. Omission is not proof that an object was deleted.
Unrelated snapshot changes do not change this applicability result. From and To
are caller labels, not proof of chronology or an ordered event replay. Selection
counts and imported bytes inherit the portable view limits; no retrieval or
authorization decision is made.

The opt-in `propagate` operation associates present historical output revisions
with caller-declared alternatives. Each alternative requires all its direct
dependencies and declared derived outputs. One intact alternative preserves
applicability. All known-stale alternatives make the output stale. Other cases,
including unresolved dependency cycles, stay unknown. Derived output links use
explicit graph edges. A relation or group selector describes selected metadata.
The operation reports each alternative and leaves both input views unchanged.
Its graph and work limits are independent of the portable view limits.

## Prior-failure claims

The `fabric.prior-failure` custom revision profiles carry versioned, inert
claim and attempt-observation data. Attempt, candidate, intended strategy,
actual artifact reference and diagnosis are separate fields. Claim bodies are
recorded outside the frozen portable basis whose digest they name; an optional
receipt pointer is an untrusted string. The projection checks the original basis
digest and the separately recorded claim before considering its selected
observation. A matching typed observation in the basis yields
`EvidenceReferenced`, not verified fact, receipt authentication or enforcement.
Missing references are unsupported, omitted, unknown or conflicting as appropriate.
A later basis cannot retroactively change an earlier claim.

Identifiers are limited to 256 bytes and receipt pointers to 1024 bytes; the
existing revision and portable-view budgets also apply. The projection does not
execute content, infer authority, or verify artifact availability.

## In-memory representation selection

A caller may select raw captured bytes, a supplied abstract, or omission for each handoff citation under a bounded byte budget. Raw bytes are checked against the captured source and citation; supplied abstracts retain their enrichment identity and remain unverified derivative claims. Omission and dissent markers remain explicit. This selection does not authorize expansion or establish custody.
