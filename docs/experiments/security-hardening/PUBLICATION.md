# Security hardening specification: repository handoff

**Status: proposed specification, not implemented security repairs.**

This directory publishes the previously prepared security change specification as a reviewable, repository-native handoff. It does not change production Rust code, CI workflows, authorization rules, or the active verification registry.

## Start here

Read the [implementation handoff](AGENT-START-HERE.md), then the [change specification](SECURITY-CHANGE-SPEC.md). The [original-source edit map](original-source-edit-map.md) and [machine-readable manifest](change-manifest.json) define 24 source-level operations against 12 pinned source blobs. The [acceptance catalogue](acceptance-tests.md) and [JSON catalogue](acceptance-tests.json) define 53 proposed checks and the defective implementations they must reject.

The reviewed application base is `18f6b367d0f34886847ea25b9168e50eb0425300`. Line numbers belong to that base, not to a future implementation. Before implementation, inspect current repository instructions and use the read-only drift check from the repository root:

```sh
python3 -B docs/experiments/security-hardening/verify_base.py .
```

A changed source requires review and re-anchoring, not an overwrite. Implementation, policy/checker changes, and their evidence must remain separately attributable.

## Scope and iteration

The proposed work covers peer-aware console admission, finite local OTLP I/O and queue waits, all-component selected-log path protection, and a diagnosed repair of Kani/extended-verification setup. Follow the specification's R0 through R5 sequence: freeze expectations, reproduce counterexamples, compare candidates, reject unsafe or dominated mechanisms, and rerun integration gates. The [decision ledger](decision-ledger.json) records design screening; no application candidate is already accepted by this publication.

TCP deadlines do not establish isolation from a continuously reconnecting hostile local process. Peer quotas do not separate clients sharing the same peer identity. Symlink rejection does not authenticate file provenance or protect against the OS owner. These limits must remain visible in implementation claims.

## Provenance and validation

The 21 original bundle files are preserved byte-for-byte. Their complete original Git tree is `45bc3fa8596b9c930f7968365f26f0f1ea2b6e7d`; this publication note is additional. [SHA256SUMS](SHA256SUMS) lists the original file contents, excluding the checksum file itself and this note.

The original documents and JSON records containing `github_mutations_performed: false` or statements that no repository writes occurred describe the earlier specification-generation session. They are historical records, not a statement that this publishing commit does not exist. The preserved [design probe results](design-probe-results.json), [helper self-test record](manifest-checker-selftest.json), and [failed initial probe](probes/rejected/v0-failure.json) are not newly run application verification.

Publication checks were limited to archive/checksum integrity, JSON parsing, Python syntax inspection, and matching the uploaded original-bundle Git tree with the local artifact. No FabricO11y build, application test, packaged-browser run, proof run, deployment, or cloud security scan was performed for publication. The [final evidence template](evidence-receipt.TEMPLATE.json) deliberately remains `not_run`; the [schema](evidence-receipt.schema.json) validates evidence shape rather than truth.

These mixed specification, machine-input, and regression-design records stay versioned with code under the [documentation ownership policy](../../documentation-policy.md#canonical-ownership). New research reports and experimental interpretations belong in the research wiki. Do not overwrite these historical results when evaluating an implementation. Run actual project tests, experiments, and validators through the repository's existing resource launcher and preserve failed-run evidence.

Return to [architecture documentation](../../README.md).
