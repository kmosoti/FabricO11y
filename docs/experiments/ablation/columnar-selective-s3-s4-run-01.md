# S3/S4 correctness: hybrid Parquet and optional postings

Status: implemented and correctness-reviewed. Formal [cost results](../benchmarks/research-costs-run-01.md) follow the
[registered comparison](../benchmarks/columnar-selective-s3-s4-protocol.md); this correctness
record does not select a physical layout. The [API](../../../tools/layout-probe/API.md) fixes
the hybrid schema, exact predicates, trusted builder and external anchor assumptions.

Two independent candidates passed the six frozen tests and two additional parser/
projection tests. Candidate A was selected after these equal gates because it had
the smaller implementation. Its `lib.rs` SHA-256 is
`0ec0c5be928706d182f9c6d09381fe16b3153966713997db0cba118ffb69a525`.
The integrated release package test command exited 0 with all eight tests passing.
After formal cost measurement, rustfmt reordered imports only; the
[adaptation record](../benchmarks/data/research-costs-run-01/rustfmt-adaptation.json)
retains both hashes and verifies the implementation body is unchanged. The eight
tests passed again on that final formatted source.

The frozen suite checks Arrow types, Parquet metadata and row groups; exact Event
round trips including floating-point bits; physical duplicate positions; inclusive
time/tenant/token predicates; authentication on every query path; deterministic
postings rebuild; and malformed/absent/wrong-table index fallback. Supplemental
tests recompute an index hash around malformed JSON to test syntax independently
of its digest. A separate rewritten raw column demonstrates that projection does
not decode raw Events; its newly minted table hash is explicitly outside the
original-anchor integrity claim.

An independent GPT reviewer executed seven IEEE bit-pattern round trips and 2,520
query combinations across full, projected and postings paths against a scalar oracle.
The clean command exited 0. Disabling table authentication or returning an empty
answer for malformed postings each caused the intended test to exit 101.
Claude executed six independent probes covering boundaries, exact floats/attributes,
nine malformed-index cases, table anchor mismatches and the builder trust boundary;
its command exited 0. Both reviews approved the selected source. Claude's attempted
shell variants outside its exact-command allowlist were denied; its own permitted
probe command completed, while the parent and GPT executed the frozen tests.

Commands, hashes, candidate results, probes and review limits are in the
[run artifacts](data/columnar-selective-s3-s4-run-01/). These finite checks rely on a
trusted encoder and an independently retained anchor. Projection still hashes the
entire file. A caller that replaces bytes and supplies a newly computed expected
hash is outside the integrity claim. The full raw column duplicates predicate
fields; any cost comparison must include it, the external anchor and index digest.
No object-store, cold-cache or physical-device result follows from these tests.
