# Range evidence and ownership of unfinished work

Status: research direction derived from the
[executed transition investigation](../experiments/benchmarks/catalog-transition-ownership-findings.md).
The three fixes exist. The subsequent [range-evidence
investigation](../experiments/benchmarks/catalog-range-evidence-findings.md)
implemented typed timestamp reduction and borrowed raw visitation, with exact
copy-allocation predictions. Its original two-cell speed threshold did not
reproduce. The indexes/controllers below remain candidates; none is selected.

## The problem the counterexample exposed

A physical Segment can grow to contain groups 1 through 3 while an older page
still refers to groups 1 through 2. Correctly filtering result rows is
insufficient: receive bounds and freshness must describe that same logical
population. The original failure returned all four correct rows while changing
freshness from 200 to 900. The independent oracle rejected the answer envelope.

Represent the evidence for a group range S as:

`E(S) = (minimum_receive_time, maximum_receive_time, maximum_signal_time_by_node)`.

Disjoint adjacent ranges can combine their evidence by minimum, maximum and
per-node maximum. With an explicit empty identity, that merge is associative.
It is not invertible: knowing a whole Segment's maximum and its later suffix's
maximum does not recover the earlier prefix's maximum. A prefix-max table alone
therefore cannot answer arbitrary clipped ranges by subtraction. This is the
structural reason the original whole-manifest shortcut was unsound.

Storage can expose two different kinds of knowledge through a narrow interface:

- Conservative bounds used to decide which physical sources might matter.
- Exact range evidence used to describe what a logical snapshot contains.

An upper bound safe for pruning is not automatically an exact freshness value.
Neither metadata kind is permission to ignore missing custody bytes or revive
a page after retention. Those availability decisions remain explicit.

## Query lab: compare genuinely different ways to obtain evidence

| Candidate | Mechanism | Cost or failure to discriminate |
| --- | --- | --- |
| Current baseline | Verify every raw Batch, borrow its fields and reduce fully decoded signal timestamps for included groups | Avoids raw copies/query Rows but still repeats reading, hashing and typed decoding per page |
| Projection scan | Read group, node and signal-time columns | Not an eligible replacement under current boundary checks: projected timestamps alone do not preserve included typed-decoding errors or excluded raw digest detection |
| Bounded reuse | Cache exact boundary evidence keyed by immutable source identity and group range | Helps repeated pages; must account cache bytes and recheck retention instead of turning a cache hit into a lease |
| Range summary tree | Merge exact summaries of disjoint covered subranges | Makes arbitrary boundaries cheap; per-node maps can multiply storage/build cost and must have a hard byte budget |

The executed follow-up replaced the initially proposed column comparison with
typed reduction and then borrowing because source inspection exposed those
integrity constraints. Removing row construction improved the isolated decoder;
removing raw copies then matched a byte/call model exactly, with unchanged peak
heap and little timing effect. Next separate reader, hash and typed-decode costs
before choosing another algorithm. Hold producer bytes, snapshot cuts and
complete answers fixed, and vary payload size, row count, node count, boundary
position and page count independently where feasible. Include cold readers and
mixed ingestion before deriving service capacity. A faster first page alone
cannot nominate a candidate.

Only if repeated range work remains material should the lab compare reuse and
trees. A tree whose node maps grow with every historical identity fails the
boundedness objective even if query time improves. Storage owns source identity,
invalidation and range evidence; query owns predicate semantics and result order.

## Boundedness lab: account work that outlives its caller

The existing fix makes an admission permit follow actual blocking work. That is
the baseline for any adaptive policy: cancelling a client does not cancel a
started blocking scan, so it cannot erase that scan from capacity accounting.
Similarly, an accepted durable submission cannot disappear merely because its
reply receiver did.

Treat body buffering, queued bytes, active commit groups, sealer working sets and
active query scans as separate owners. Their costs overlap in time. A controller
must observe those costs and rejection/retry behavior together, rather than
multiply isolated throughput wins. Fixed independent pools remain a useful
control arm because they are explainable and do not need a fitted cost model.

Compare them with estimated-work admission only after measuring small selective,
broad and rate-query demands mixed with ingestion. Charge estimates until work
finishes, reconcile against observed work, and retain hard limits independent
of prediction quality. Reject a policy on starvation, increasing resource debt,
incorrect custody, or worse ACK/query tails outside the registered margin.
Body deadlines and authentication before expensive ownership are necessary
separate pressure cases; the current pool test does not establish them.

## Lifecycle lab: make uncertain outcomes safe to retry

The next deterministic cut is after Intake accepts exact bytes and before the
caller sees an ACK. Hold the worker at that observed boundary, cancel the caller,
release the worker, stop/reopen and retry the same bytes. The independent producer
ledger must still match recovered custody exactly, and the unchanged delivery
oracle must accept the trace. Repeat with rejection before admission and with
retention after publication. Record which boundary was actually crossed; a
wall-clock sleep followed by disconnect is not that evidence.

## Why the combination is useful

A human investigating an incident and an agent making a follow-up query need the
same stable facts: the data population queried, whether it was complete, how fresh
it was, and whether more work was admitted. They can use the existing structured
answer envelope and explicit overload response without a second telemetry model.

The proposed direction is a storage map that returns exact evidence for logical
ranges, paired with execution that keeps ownership of unfinished work. Success
means cheaper answers with unchanged meaning and predictable pressure behavior.
It is an experimentally testable design direction, not a claim of novel prior
art, universal optimality or deployment readiness.
