# Fixtures

Fixtures are conformance evidence, not disposable samples. The MVP includes:

- canonical valid and invalid contract records;
- retraction, replacement, qualification, and deduplication scenarios;
- frontier gaps, retention boundaries, sampling, and missing producers;
- sealed segments, missing dictionaries, truncation, and corruption;
- privacy-preserving small structured records for compression evaluation.

OTLP-derived and broader multi-producer evaluation corpora remain downstream work.

Sealed compatibility inputs live under `fixtures/golden/` and byte-format inputs under
`fixtures/segment-format/`. Golden fixtures carry provenance manifests and must not be overwritten
once referenced by a compatibility result.
