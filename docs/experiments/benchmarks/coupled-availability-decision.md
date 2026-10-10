# O6 projection availability decision and companion checker

Status: **owner-authorized semantic implementation; prepared, not tested**.
Root registers the trust-boundary checker/fixture change separately from production
implementation. Existing `query_oracle.py`, policies and historical failures remain
unchanged. [Preparation](coupled-operations-completion-readiness.md) explains the
source/model disagreement and the required semantic scope.

## Narrow decision

A missing requested projection excludes that source's projected rows and marks
matching-source completeness false. Verified Manifest window/freshness and verified
gaps survive; they are graded from independent pre-IO producer bytes, not Rust
output or Manifest expectations. Other projections' rows survive their unrelated
table loss. Storage unavailability is not a collection gap.
When raw Batch custody is unavailable but projections/Manifest/gaps survive,
projected rows and known metadata survive, while matching-source completeness is
false. A complete projected answer in that fixture is an implementation defect.
This does not claim current custody remains readable after raw loss.

The companion requires explicit `{raw_available:bool,missing_projections:[...]}`.
False currently means ALL raw sources in the fixture are unavailable; partial raw
loss is outside this narrow checker. Projection selectors name logs/metrics/spans
plus either node/sequence or a half-open receive interval. Rate uses metrics.
Raw-loss scope is restricted to a nonempty matching producer population; empty,
absent-node and no-match queries explicitly return MALFORMED (tested), rather than
claiming the candidate's global raw-loss completeness behavior. That behavior needs
a separate decision. Projection-loss completeness keeps the original independent
could-match interpretation. Duplicate identities use actual producer Strand
(node_id,generation,sequence), not label/sequence across generation resets.
Manifest/gap availability, disk verification, HTTP Gone/token authenticity and
arbitrary corruption remain separate runtime tests, not inferred by this checker.

[`projection_availability_oracle.py`](../../../tools/qualification/projection_availability_oracle.py)
reuses the independent Python oracle's protobuf decoder, row/page/envelope checks,
without changing it. It computes metadata from all producer records; only requested
projection row selection uses exclusions. Tests use Python's independently encoded
producer fixtures, explicit known metadata assertions, and mutations. Reject false
complete, erased/invented freshness, erased retained window/gaps, dropped/duplicate/
reordered rows, page overflow/snapshot/envelope inconsistency and malformed inputs.
Strict JSON, bad selector types/intervals/duplicates and duplicate producer identity
are rejected. The checker cannot authenticate opaque tokens or stand in for Gone.

## Fixture routing and prospective commands

`completion_storage` preserves its existing partial-projection test name, table
cuts, `complete:false` assertion and all producer-derived envelope/row assertions;
it now selects this companion rather than incorrectly declaring whole-record loss.
Six fresh History queries cover3 tables ×2 plans. No expectation is derived from
what the runtime happened to return. Numbered oracle directories preserve ledger,
query, pages, availability and verdict per call with `FABRIC_STORAGE_EVIDENCE`.
Historical O6/fast failures remain failed and archived.

New `missing_raw_batches_preserve_projection_rows_but_mark_incomplete` verifies
intact Segment before deleting only `batches.parquet`, then grades complete chains
for logs/metrics/spans under both plans. Root runs it BEFORE any fix; silent complete
answers must fail the companion. Narrow production fix is coordinated separately:
use existing raw table open/size/schema/footer-row validation to mark unavailable,
keeping projected rows and known envelope facts. Full decode/hash corruption is
outside that fix's coverage and must remain explicit.

Run only through resource launcher/coordinator and registered admission; these are
exact inner commands (no execution by preparation agent):

```sh
python3 -B tools/qualification/test_projection_availability_oracle.py
cargo test --offline --locked -p fabric-server --test completion_storage --all-features missing_query_tables_are_incomplete_with_independently_declared_unavailability -- --exact --nocapture
cargo test --offline --locked -p fabric-server --test completion_storage --all-features missing_raw_batches_preserve_projection_rows_but_mark_incomplete -- --exact --nocapture
```

Freeze checker/tests/fixture/decision SHA and unchanged original oracle SHA first.
Malformed input exits2, semantic violation1, accepted complete chain0. Archive red
raw-loss outcome; run unchanged regression on separately captured fix, then full
storage file and full fast. No qualification, checksum guarantee or all-corruption
claim follows from this narrow decision. Failure remains failure; do not relax
negative controls or the independent original oracle to obtain a green result.
