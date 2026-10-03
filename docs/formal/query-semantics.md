# Query semantics: the specification and its theorems

Status: the executable specification lives in the pure core, [fabric_core::query::spec](../../crates/fabric-core/src/query/spec.rs); its theorems are properties in [query_spec.rs](../../crates/fabric-properties/tests/query_spec.rs); bounded Kani proofs were attempted and did not finish (below). This page states the same thing in mathematics so that a reader can check the code against the definition rather than the definition against the code. It does not change the [retained-history contract](../architecture/retained-history.md); it is that contract's query section written as functions.

## Why a specification and not an oracle

An oracle is a second program. When two programs agree they share whatever assumptions they share, and when they disagree nothing says which is wrong. The Python query oracle is independent of the Rust code, which makes it a useful diversity check, but it is not a definition: its correctness rests on its author having read the contract the same way. A specification is different in kind. It is the shortest text from which the answer follows, read in full and checked against the contract's words; every implementation, the Python oracle included, is then judged by refinement to it, and the question "who checks the checker" ends at a definition small enough that checking it is reading it.

The definition here is twenty lines: filter, sort by the total key, take `limit`. Its theorems are what give a reader confidence that those twenty lines mean what the contract means. Each theorem is a relation the definition satisfies with itself, so it needs no oracle at all; and each mechanism Fabric adds on the read path is proved equal to the definition rather than compared to the stock server.

## The definition

Let `R` be the finite set of log rows the retained history holds. A row `r` has a group `g(r)`, a key `k(r) = (t, node_id, sequence, index)` from a total order (lexicographic on the tuple), a node label `n(r)` and a body `b(r)`. Keys are distinct across `R` (the contract's total order).

A query `q = (w, N, S, L)` has a half-open window `w = [from, to)`, an optional node `N`, an optional substring `S`, and a limit `1 ≤ L ≤ 10,000`. A snapshot `σ = [oldest, newest]` is a range of groups. A continuation `a` is a key or none.

**Admission.** `A(q, σ, a) = { r ∈ R : g(r) ∈ σ ∧ t(r) ∈ w ∧ (N = ∅ ∨ n(r) = N) ∧ (S = ∅ ∨ S ⊑ b(r)) ∧ (a = ∅ ∨ k(r) > a) }`.

**Page.** Let `s = sort_k(A(q, σ, a))`. Then `page(q, σ, a) = (s[0..min(L, |s|)), next)` where `next = k(s[L−1])` if `|s| > L`, else none.

**Drain.** `drain(q, σ) = page(q, σ, ∅).rows ++ drain'(next)`, continuing while `next ≠ none`. It terminates: each `next` is a key strictly above the last, and keys are finite.

Metric points are the same definition with the key `(time_ns, node_id, sequence, index)`, a name filter in place of the substring, and no body. Rates are `counter_step` folded over each series' points in key order (already in the core, with its own proof).

## The theorems

Stated for all `R`, `q`, `σ` unless noted. Each is a property at 3,000 random cases on rows of at most 24. Bounded Kani proofs were attempted for T2, T7 and T8 and are **not run**: at four rows, times below 8 and limits of at most 3 the first harness had not finished after an hour, and at three rows, two groups, times below 4 and limits of at most 2 it exceeded a 40-minute budget; the symbolic sort over a vector is what bounded model checking cannot unwind cheaply here. The harnesses are not in the registered proof file, so the `kani-core` check is unchanged. An unbounded proof is a routine induction on the drain (each page removes the smallest admitted keys; the continuation is strictly greater) and is not written.

| | Theorem | Statement | Property | Proof |
| --- | --- | --- | --- | --- |
| T1 | Determinism | `page` is a function of `(R, q, σ, a)` and of nothing else; the input order of `R` is irrelevant | by construction (the definition sorts) | by construction |
| T2 | Pages partition | `drain(q, σ)` lists exactly `A(q, σ, ∅)`, each row once, in key order; it equals `page` with an unbounded limit | `pages_partition_the_answer` | attempted, not run (see above) |
| T3 | Snapshot invariance | adding rows with `g ∉ σ` to `R` leaves `drain(q, σ)` unchanged | `the_snapshot_hides_other_groups` | — |
| T4 | Limit prefix | `L ≤ L'` implies `page(q[L]).rows` is a prefix of `page(q[L']).rows` | `a_smaller_limit_is_a_prefix` | — |
| T5 | Filter restriction | `drain(q[N]) = drain(q[∅]) restricted to n(r) = N`, order preserved | `a_node_filter_restricts` | — |
| T6 | Window split | `a < m < c` implies `drain(q[[a, c)]) = drain(q[[a, m)]) ++ drain(q[[m, c)])` | `windows_split` | — |
| T7 | Threshold walk | for any partition of `R` into sources with sound bounds (`min ≤ t(r) ≤ max` for every row of the source), in any order, `walk(sources, q, σ, a) = page(q, σ, a)` | `the_walk_equals_the_definition` | attempted, not run |
| T8 | Budget boundary | for any budget `B`, `drain_budgeted(sources, q, σ, B) = drain(q, σ)`, and every budgeted page is a prefix of the unbudgeted page from the same continuation with all rows below its boundary | `the_budget_drains_to_the_definition` | attempted, not run |
| T7′ | Precondition | T7 needs sound bounds: with a lying bound the walk may skip a row, and `bounds_hold` detects the lie | `unsound_bounds_can_break_the_walk` (negative control) | — |

T7 is the theorem behind ledger L-04 and T8 the one behind L-05: the mechanisms are defined in the specification's own terms (a sorted vector for the heap, sources as sets with bounds) and shown equal to the definition. The server's implementations of the same rules are then refinements of `threshold_walk` and `budgeted_walk`, not of the stock server.

## Refinement obligations

Each layer between the journal's bytes and an answer has one obligation, stated against the definition:

| Layer | Obligation | Checked by |
| --- | --- | --- |
| Row derivation (`rows.rs`) | the rows derived from a record are the contract's rows: one per `LogRecord` in request order with the stated fields | the Python oracle's independent decoder (diversity); HIST-1 |
| Journal tail reader | `R_tail` = every row of every committed group in the snapshot | HIST-1, HIST-2 |
| Segment reader | `R_segment` = the rows of the records the Segment was sealed from; row-group pruning never drops an admitted row (sound time statistics: T7's precondition) | HIST-1, HIST-2; the pruning is an instance of T7 with one source per row group |
| Tail key index | the selected entries contain every admitted row (a sound selection) and the evidence fields are folded over the unselected ones too | run L-03's differential today; the obligation is a T7 precondition on entry bounds |
| Threshold walk | equals `threshold_walk` of the specification | T7; run L-04's differential as regression |
| Budget boundary | equals `budgeted_walk` of the specification | T8; run L-05's differential as regression |
| Evidence fields | `complete ⇔ no source that could hold an admitted row was unavailable`; the retained window and freshness are folded over every retained record, not over the selection | HIST-3; stated in the core's `complete` |

A hypothesis in the [suite](../research/hypotheses.md) that changes any layer (a new Segment layout, a block tail, a different sort) carries that layer's obligation: it is shown to produce the same `R` as before, or to refine `page` directly, and the stock server is not consulted.

## What this does not cover

- The derivation of rows from OTLP bytes is specified in prose in the contract and checked by the oracle's independent decoder; a formal definition would be a grammar over the protobuf fields, which is larger than this page and is not written.
- Metric points and rates reuse the definition with a different key and filter; the executable specification states the log case and `counter_step`; a metric instance of `spec` is a mechanical copy that has not been written.
- The properties are random (24 rows), not a proof for all sizes; the bounded proofs did not finish. The definition is simple enough that an unbounded proof would be a routine induction on the drain; it is not written. A proof-friendly restatement of the definition (an array of fixed capacity in place of the vector, a selection sort written as a loop) is the way to make the bounded proofs finish, and would be a change to the specification's form, not its meaning.
- Concurrency (a page read while the sealer moves rows from the tail to a Segment) is the snapshot's job and is modelled in prose (HIST-4), not here.
