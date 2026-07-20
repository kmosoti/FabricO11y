# Schemas

This directory is the source of truth for FabricO11y's versioned public envelopes. FO01 will define:

- `fabric.observation/v1`;
- `fabric.correction/v1`;
- `fabric.frontier/v1`;
- `fabric.answer/v1`.

Each schema change must include canonical valid examples, invalid fixtures, documentation updates,
and compatibility analysis. A record has one explicit epistemic class; consumers must not infer a
mixed state from payload shape.

Storage manifests and dictionary registry contracts are added with FO03 after the evidence
envelopes are stable.
