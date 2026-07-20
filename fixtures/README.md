# Fixtures

Fixtures are conformance evidence, not disposable samples. Planned families include:

- canonical valid and invalid contract records;
- partial-order multi-producer histories with clock skew and late arrival;
- retraction, replacement, qualification, and deduplication scenarios;
- frontier gaps, retention boundaries, sampling, and missing producers;
- sealed segments, missing dictionaries, truncation, and corruption;
- OTLP-derived traces and logs;
- privacy-preserving small structured records for compression evaluation.

Generators will live under `fixtures/generators/`; sealed compatibility inputs will live under
`fixtures/golden/`. Golden fixtures must carry provenance manifests and must not be overwritten once
referenced by a compatibility result.
