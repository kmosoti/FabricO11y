# D-0217: Versioned trace parents

Keep the strict v2 and v3 wire shapes while adding an opt-in trace envelope
under a separate retained descriptor. Version 4 uses explicit trace and span
IDs, with a single optional parent in the same trace. Legacy projected span
IDs stay valid within their original contract.

Identifiers use fixed-length nonzero lowercase hex. Validate the raw tokens
under the frame and nesting bounds before building owned IDs. Trace rows keep
the wire version, producer metadata and any available source provenance. A
parent reference can precede its parent row. Duplicate claimed span IDs remain
visible because their presence cannot prove a single matching parent.
