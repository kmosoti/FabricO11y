# D-0205: Selected-head reconciliation

Analyze the immutable as-of selection and append a child only when the selected
ancestry is complete and the child names exactly the competing selected heads.
This preserves historical revision bodies and attributed relations. Unknown
selected records and unselected ancestry prevent a completeness claim. The
selection is not a global authority or an atomic conditional write; adapters
that require global concurrency control must provide it separately. Bounded
link and missing-parent aggregation keeps the pure operation finite.
