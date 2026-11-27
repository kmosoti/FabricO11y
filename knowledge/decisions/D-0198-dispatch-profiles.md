# Operation dispatch profiles

The architecture check verifies native x86-64 and wasm32 normal and build
closures for `fabric-ops`, `fabric-colloquium` and `fabric-corpus` against
explicit transitive allowlists. An added dependency requires review and a
policy update. These checks do not establish feature profiles or boundaries
for the larger Arrow-backed crates.

The dedicated query-dispatch benchmark uses seven real query operation variants
and the existing `QueryRequest` and `QueryResponse` types. Static generic,
closed enum and dynamic trait-object paths call the same typed handler and
compare complete output digests. Fixture construction and output verification
are outside the timed loop; response allocation and result extraction remain
inside it. The corruption control must fail output comparison. Results apply
only to the recorded workload and compiler, hardware, profile and resource
limits; a comparison does not choose a production dispatch architecture.

Process-boundary negotiation and its compatibility remain separate from this
measurement. The benchmark grants no execution authority and does not alter
existing request schemas or runtime dispatch.
