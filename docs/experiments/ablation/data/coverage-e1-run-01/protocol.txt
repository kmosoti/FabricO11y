# E1R: coverage receipt correctness

Status: registered before implementation or candidate evaluation. This is the first
coverage increment following [S1](storage-query-s1-run-01.md), not an application
query service. Resumable answers, one-sync commits and cost comparisons are separate
increments. This concretizes the externally reviewed Claude study's E1R proposal;
the complete repository-native contract is below.

## Claim and trust boundary

Given immutable event rows, a correct summary builder, an independently retained
snapshot anchor, collision-resistant SHA-256 and a trusted exact scan executor,
every verified complete receipt accounts for every block, and each excluded block
has no matching row. A valid receipt with an unavailable candidate block is
incomplete. Invalid metadata is an error, never an empty answer. Result identity is
input position, not EventId; results retain arrival order and duplicates.

The Rust idea is separation of authority through types and ownership: a sealed
snapshot privately owns its rows; the verifier takes only an anchor, query and
receipt. `Result<CoverageStatus, CoverageError>` separates invalid input from a
valid incomplete answer. No row data or callback is available to the verifier.

For this first logical experiment use exact ordered tenant/token sets and min/max
event time. This differs from S1's Bloom representation, makes summary validity
inspectable, and makes no claim about memory or speed. Summaries may be conservative
supersets. Every row time, tenant and exact `split_whitespace` Log token must be
represented before sealing. Gauge rows contribute tenants/time but no body tokens.
Empty or multiword query tokens match no single whitespace token. Time is inclusive.

Metadata commitments bind block ordinal, start position, length, full row digest,
summary digest, and a fixed format/tokenizer version. A domain-separated Merkle
root binds the ordered commitments; the anchor also binds snapshot identity,
block count and row count. Odd tree levels duplicate the last node; proof length
and orientation follow the trusted block count and ordinal. Verification checks
contiguous nonempty block ranges, exact ordinal coverage, paths, query and snapshot
identity, summary digest and the exclusion predicate. Empty snapshots are valid.

A scanned disposition does not prove execution or returned-row completeness.
A builder version does not prove summary correctness. Deliberately bypassing the
builder to authenticate `row=rare, summary=common` must demonstrate that a row-free
verifier can accept the incorrect exclusion; only the independent row oracle sees
the semantic error. This is an explicit limit, not a cryptographic fix.

## Fixed workload and oracle

- Four S1-shaped corpora: Gauge, clustered Logs, shuffled Logs and mixed.
- Seeds 101, 102, 103; 2,048 rows each; blocks of 64; 128 queries per snapshot.
- Query generator seed 7; pin the generator and independent positional full-scan
  oracle before candidate implementation. Include full/narrow/inverted time,
  tenant, absent/common/rare token and combined predicates.
- Additional small cases: empty/one/odd block counts, timestamp extrema, Unicode
  whitespace, duplicate IDs, conservative summaries, all Scalar variants, float
  bit patterns and changed attributes. Row hashing covers the entire Event, not
  merely searchable fields, with tags, lengths and raw float bits.

For each corpus/query, clean results must exactly equal the independent full scan.
Inject omitted ordinal, stale summary, swapped order, replaced anchor root,
snapshot mismatch and truncated block count. Exercise unavailable blocks separately:
a missing candidate is explicit, while a block safely excluded by trusted metadata
need not have readable raw rows for this query. Reject invalid builder summaries
for time, tenant and token omission. Preserve the sealed-semantic-bug negative control.

Metric: `contract_violations`; threshold **0**. No timing winner or overhead gate
is selected here. The finite corpus is evidence, not a proof of arbitrary execution.
The conditional argument is by partition: every block is accounted for; a scanned
block is evaluated exactly by assumption; a validated excluded block cannot match;
an unavailable candidate prevents completeness. Hash binding prevents metadata
substitution under the stated collision assumption, not a dishonest builder.

## Checks and non-vacuity

An independent oracle author writes the contract tests and fixed corpus before
implementation. Two isolated candidates are selected using those tests, then
independent probes and cross-family review challenge the selected implementation.
The frozen checks must fail when root binding is bypassed or builder validation
is bypassed; retain mutation outputs and real nonzero exits. The deliberately
faulty chain-only baseline also demonstrates missing trust-root enforcement.

Commands from the repository root:

```sh
cargo test --offline --locked --manifest-path tools/storage-probe/Cargo.toml
cargo test --offline --locked --manifest-path tools/storage-probe/Cargo.toml --test coverage_e1 -- --nocapture
```

Preserve raw command outputs, toolchain, source hashes, preimplementation oracle
hashes, candidate results, mutation diffs/exits and review outcomes in the run record.
No FOL2 bytes, append semantics, application CLI or network protocols change.

## Related

- [Research package](../../../tools/storage-probe/README.md)
- [Storage research agenda](observability-storage-research.md)
- [Current system](../../architecture/system.md)
