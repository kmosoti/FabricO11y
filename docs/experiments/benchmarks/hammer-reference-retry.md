# Native reference literal correction

Before retrying the [registered reference workload](hammer-reference-protocol.md),
`references-01` failed on the first Vortex predicate: unsigned `observed_ns`
was compared with an implicitly signed Python integer literal. Vortex 0.87.0
rejects that comparison. The exact failed source, expected rows and artifact
state remain in the launcher failure tree; this is a harness integration error,
not a Vortex correctness or performance result.

The correction uses the installed API's explicit `literal(uint(64), value)`.
No input schema, workload, expected answer, engine version or measurement rule
changes. `references-02` is a fresh recorded attempt. Successful source/result
hashes and the original failed inputs must be preserved before cleanup.
