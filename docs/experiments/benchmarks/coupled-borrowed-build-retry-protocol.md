# C4 build retry and observation disposition

Status: registered before retry. Initial diagnostic 01 failed before measurement:
the pinned OTLP KeyValue type requires `key_strindex`, omitted by the new fixture
initializer. Add `..Default::default()` so this unused field is zero, preserving
the registered eight string attributes and wire interpretation. No product source,
oracle, workload, size, timing rule or CR2 result changes.

The initial coordinator snapshot also preceded the PI's last helper additions;
run 01 remains failed with that provenance limitation, and no native result is
inferred. All source edits are now frozen. Retry uses new coordinator ID
`catalog-coupled-c4-diagnostic-02` and new dataset
`catalog-borrowed-attribution-run-02`, otherwise the previous registered command.
The compile failure is preserved and its owned scratch archived/byte-verified.

Clarify diagnostic disposition: a negative phase residual beyond the registered
tolerance rejects **phase attribution**, retaining already collected semantic
results with an explicit inconclusive attribution field. This does not accept a
bad residual or nominate a candidate; full-chain, association, resource and
provenance failures still stop the run. Observer ratio misses likewise retain
semantic collection while withholding causal conclusions. Original initial
wording and failure remain historical, no completed measurement is reclassified.
