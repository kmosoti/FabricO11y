# D-0203: Bounded lexical rebuild

Derive directed adjacent-token counts and revision provenance from the explicit
as-of membership rather than maintaining a second canonical authority. The
ASCII tokenizer and hard budgets are specified in the corpus contract. This
keeps rebuilding deterministic across native and wasm targets and preserves
missing selected revisions. A locale-aware tokenizer would need a separately
versioned contract and new fixtures. Authorization, incremental indexing and
wiki projections remain outside this decision.

Attribute-preserving backlinks use the exact selected relation digests and an
explicit predicate rather than token proximity. The frozen projection retains
selected-missing relations and distinguishes missing or omitted endpoints.
