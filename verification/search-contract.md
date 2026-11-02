# Algorithm evaluation contract

Candidates implement `pub fn solve(data: &[u64], queries: &[u64], k: usize) -> Vec<u64>`
using Rust's standard library. Inputs are immutable. All preprocessing and output
allocation count toward runtime. Functions must terminate without panic for valid
inputs. They may not perform I/O, inspect their environment or use unsafe code.

- Membership: return 0 or 1 for each query in its original order; duplicates in
  data do not change membership. Ignore k.
- Top-k: return the largest min(k, data.len()) values in descending order,
  preserving duplicates. Ignore queries.
- Intervals: data is an even-length list of closed (start, end) pairs with
  start <= end. Return the sorted, disjoint union as flattened pairs. Merge
  overlapping and endpoint-sharing intervals, not separated integer neighbors.
  Ignore queries and k. Empty inputs yield empty outputs.

The development and held-out evaluations use different deterministic datasets.
Evaluation timing includes process startup and sandbox overhead and must be
interpreted accordingly; it is not a claim about isolated instruction cost.
Use repeated runs and competent baselines before selecting an implementation.
Each invocation warms the solver three times before timing repeated complete calls.
Query counts and k vary independently of data generation; ordered, reversed,
duplicate-heavy, disjoint and integer-boundary cases are included. Peak resident
memory includes the process, input parser, inputs and outputs, in KiB; it is not
an isolated allocation count. Compute time includes preprocessing on every call.
Report each workload separately, including memory and elapsed process time.

Candidate manifests record task, strategy, trial number, algorithm family,
rationale, optional parent identity, and source path. Sources are registered as
immutable copies with a contract digest. Failed evaluations remain evidence.
Record algorithm, representation, ownership, data flow and a falsification
experiment explicitly. Family labels are declarations to review, not automated
evidence of structural novelty.
