# D-0194: Causal contribution projection

- Date: 2026-11-27
- added_by: agent
- human_ratified: false
- Status: active

Use a dependency-free Sans-I/O crate for typed attribution and bounded causal
contribution state. The corpus crate owns research revision identities; a
contribution ID does not imply a corpus revision or custody. Actor, principal,
and optional team are distinct opaque claims. Custom kinds are inert.

Direct proposal parents identify the proposal heads replaced by a proposal.
Concurrent proposals based on the same parent remain heads even when processed
sequentially. Parent-first admission establishes an acyclic graph and turns are
one plus the maximum parent turn, with roots at zero. Explicit record, parent,
head, and custom-name limits bound the projection; failed appends do not mutate
it. A caller must separately bound decoded input and out-of-order staging.

An opt-in review profile can seal caller-selected admitted IDs into immutable
boundaries. Late arrivals stay out of a sealed view until explicit reopening,
which retains prior boundaries and requires the same limits and an exact
extension of prior records. Reused IDs with changed content are rejected. Total
input and reopening caps are independent of contribution turns; exhaustion
remains unresolved and creates no decision.
The profile imposes no fixed rounds or global turn order and does not add
acceptance authority.

A single-head model would discard concurrent alternatives; inferring replacement
through arbitrary discussion ancestry would require an additional semantic rule.
This projection is not authentication, durable deduplication, acceptance,
policy enforcement, or external execution authority. Revisit those boundaries
when conditional transactions and durable recovery are specified.
