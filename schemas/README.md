# Schemas

This directory is the source of truth for FabricO11y's versioned public envelopes. The MVP defines:

- `fabric.observation/v1`;
- `fabric.correction/v1`;
- `fabric.frontier/v1`;
- `fabric.answer/v1`.

Each schema declares JSON Schema Draft 2020-12 and a stable HTTPS identifier. A schema change must
include canonical valid examples, invalid fixtures, documentation updates, and compatibility
analysis. A record has one explicit epistemic class; consumers must not infer a mixed state from
payload shape.

Storage manifests and dictionary registry contracts are defined separately because they describe
physical evidence containers, not domain meaning. Run `uv run --no-project
scripts/validate_contracts.py` from the repository root to validate every schema and fixture.
