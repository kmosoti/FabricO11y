# FabricO11y security change specification bundle

Start with `AGENT-START-HERE.md`, then `SECURITY-CHANGE-SPEC.md`.

The bundle contains 24 original-source operations, 12 pinned Git blobs, 53 acceptance cases, a candidate decision ledger, a read-only drift checker, a final evidence schema/template, and reproducible isolated design probes.

Application implementation and application verification have not been performed. The included results concern only standalone design probes and the bundle helper. Historical failed probe evidence is deliberately retained.

`SHA256SUMS` records the delivered file identities, excluding itself. `build_bundle.py` regenerates only the manifest and test maps; it is not an application patch generator.
