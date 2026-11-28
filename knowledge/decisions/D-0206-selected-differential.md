# D-0206: Selected dependency differential

Compare explicit dependencies in two caller-bound portable snapshots. Preserve
selected-missing and omitted states instead of treating selection as evidence of
deletions. A known selected change is stale; otherwise missing or omitted inputs
are unknown. The result is reproducible from verified portable bytes, but does
not establish custody, authorization, chronological order or a global view.

Explicit derivative dependencies can be projected over the same frozen inputs
as bounded alternatives. Start unknown and propagate known outcomes until the
selected graph settles. This preserves an intact alternative and leaves an
ungrounded cycle unresolved. Attributed support remains a separate claim from
an explicitly declared derivative dependency.
