# S3/S4 oracle open questions

These points are not determined by `API.md` and the registered protocol. The
black-box tests do not impose an answer.

- What concrete `io::ErrorKind` or message identifies each rejection or fallback?
  The contract requires `Err` or exact fallback results, not a particular error
  classification.
- What are the JSON field names, nesting, and token-list ordering inside the
  postings file? The contract requires deterministic JSON, normalized duplicate
  names, sorted unique positions, and a full binding, but does not publish a
  JSON schema. The tests use malformed and stale whole indexes without editing
  assumed fields. Field-level invalid version, row count, and position probes
  need a registered JSON schema or a test-only documented fixture.
- Does `build_postings` reject a table anchor with a wrong version or hash when
  `rows` has the right count? It cannot authenticate the table file from the
  arguments it receives. The tests assert only the specified row-count check.
- Is `encode` required to yield byte-identical Parquet files on repeated calls?
  Determinism is specified for postings JSON, not table bytes.
- What row-group count is required for an empty table? The contract requires a
  valid empty table and a caller-supplied row-group limit, but does not mandate
  a zero-row group or zero row groups.
- How should a test construct a trusted anchor for a malformed yet hash-matching
  Parquet file? The trusted encoder is the only specified anchor producer, and
  a caller supplying a fresh hash for edited bytes is explicitly outside the
  security claim. The current black-box tests cover mutations with the retained
  anchor; they do not assert rejection of a forged matching anchor.
