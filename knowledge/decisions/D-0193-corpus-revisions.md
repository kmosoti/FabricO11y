# D-0193: Research corpus revision identity

- Date: 2026-11-27
- added_by: agent
- human_ratified: false
- Status: active

Use a dedicated Sans-I/O crate for research record contracts, independent of
Arrow and the collector. Logical IDs remain separate from immutable revision
digests; admitted parent references establish a same-object DAG. Attributed
relations and postcommit attestations remain separate claims. Captured and
external-only source references do not establish custody or availability.

A versioned length-prefixed binary profile avoids JSON number and duplicate-key
ambiguity in the hash boundary. Its exact byte layout and limits are documented
in the corpus contract. Adapters must bound and validate their own input before
constructing values. A module in fabric-core would reduce workspace churn but
would carry its larger dependency closure to native and Wasm consumers. This
choice does not establish durable result compatibility, serialization, source
availability, or independently verified custody; revisit these boundaries when
those adapters and contracts are implemented.
