# Glossary

This glossary defines the vocabulary used by FabricO11y contracts, code, tests, and documentation.
Normative schemas may narrow a term further but must not silently change its meaning.

## Evidence terms

| Term | Meaning |
|---|---|
| Observation | An admitted record of something a producer emitted or a collector observed. Admission preserves origin and integrity metadata; it does not endorse the payload's interpretation. |
| Epistemic class | The declared relationship between a record and what it claims to establish. One record has one class; classes must not be blended implicitly. |
| Observed fact | Evidence that a concrete emission, state reading, or action report occurred. It is factual about the observation, not necessarily about an imported conclusion. |
| Derived conclusion | A result computed from identified inputs and a named derivation. It must retain provenance and remain revisable. |
| Correlation | An explicit association between records that does not, by itself, establish causation or truth. |
| Assumption | A declared premise used when evidence is absent or a derivation requires a condition not established by observations. |
| Provenance | The records, producers, fields, transformations, and derivation steps that support a result. |
| Integrity | Evidence used to detect accidental or unauthorized change, such as a digest or checksum. Integrity does not establish semantic truth. |
| Authority | The scope in which a source is permitted to make a claim. Imported authority must be explicit and does not transfer automatically to derived conclusions. |
| Classification | A policy label governing persistence, redaction, access, retention, and export. |
| Payload reference | A stable reference to content stored inside or outside an observation head, normally paired with integrity metadata. |

## Change terms

| Term | Meaning |
|---|---|
| Correction | An append-only record that changes how one or more prior observations should be interpreted. |
| Retraction | A correction operation declaring that targeted evidence must no longer support active derived state. The original record remains auditable. |
| Replacement | A correction operation superseding targeted evidence with explicitly identified replacement records. |
| Qualification | A correction operation narrowing or adding conditions to the interpretation of targeted evidence without deleting it. |
| Deduplication | A correction operation identifying multiple records as representations of the same underlying occurrence. |
| Replay | Deterministic reconstruction of state from admitted observations, corrections, and relevant metadata. |

## Source and completeness terms

| Term | Meaning |
|---|---|
| Producer | An identified source that emits observations. |
| Stream | A producer-local ordered sequence or logical channel. |
| Producer sequence | A monotone producer-local position used for ordering and gap detection. It does not imply order across producers. |
| Frontier | The latest completeness claim known for a producer and stream, including known gaps, retention limits, sampling, and producer state. |
| Coverage | A query-time characterization of how completely the available evidence covers the requested sources and interval. |
| Missing source | An expected producer or stream for which sufficient evidence or frontier information is unavailable. |
| Source cutoff | The evidence and frontier boundary at which an answer was evaluated. |
| Conflict | Two or more applicable records or derivations that cannot be reconciled under the current rules. |
| Answer | A query result plus its provenance, frontiers, coverage, assumptions, conflicts, missing sources, derivation, and evaluation cutoff. |

## Time and relation terms

| Term | Meaning |
|---|---|
| `observed_at` | Time reported by the source or origin clock. It may be absent or unreliable. |
| `observed_by_at` | Time at which a collector or runtime observed the record. |
| `recorded_at` | Time at which FabricO11y durably admitted the record. Admission requires this value. |
| Logical time | A non-wall-clock ordering token, such as a Lamport-style value, used to preserve partial-order information. |
| Parent relation | An explicit causal or structural parent edge. |
| Link relation | An explicit causal or dependency association that is not equivalent to parentage. |
| Correlation relation | An explicit association that carries no causal implication on its own. |
| Partial order | An ordering in which some pairs are comparable and others are concurrent or unknown. FabricO11y does not manufacture a total order from timestamps alone. |
