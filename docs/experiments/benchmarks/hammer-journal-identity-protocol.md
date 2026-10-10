# Journal identity repair and service confirmation

The [error-body diagnostic](hammer-walk-errorbody-protocol.md) repeated nine
HTTP 500s carrying `failed to fill whole buffer`. The separate deterministic
`tail-identity-baseline-01` command compiled and ran four real-journal cases:
three failed by newer-Batch substitution, out-of-range-offset arithmetic panic
and discovery identity substitution. The same-file CRC-corruption control passed.
These are demonstrated defects; correspondence to every original HTTP error
remains an inference until instrumentation or confirmation narrows it further.

Implement a narrow storage repair: discover active before sealed files; verify
the expected first group on the handle used for offset reads; follow the
immutable sealed name after rotation; reject a shortened verified file or
mismatched requested group. Keep handles scoped to individual operations rather
than retaining deleted journals for whole queries. Preserve checksum/decode
errors, current query retry count, exact source/query oracles, wire format,
durability/sync ordering, query defaults and resource limits.

Run the same four regression assertions after the patch. Run existing catalog
discovery, transition and full fast checks. Freeze and build the new Rust source
under a fresh build-manifest path; never overwrite the prior manifest or label
its old benchmarks as results for the repaired revision. Register explicit
build-manifest CLI selection in the research harness, leaving the historical
default path intact. Keep the baseline failure fixtures and exact commands.

If regressions pass, run fresh 4 GB service cells with the original twenty-node
1k/4k/16k source stages and quiet interval. Order: Walk seed 2704101, Scan seed
2704101, Scan seed 2704102, Walk seed 2704102, admitted one at a time within the
existing remaining round allowance. Capture bounded HTTP error bodies for each.
Every original semantic/drain/live-query gate must pass; a lower memory or CPU
cost cannot compensate for a failed gate. Stop and investigate another semantic
failure. Report producer lag, source/Spool/ACK rates, query and visibility latency,
RSS and cgroup memory separately. The old halted pair remains failed and is not
part of a repaired-code speed nomination.

The hypothesis is identity-safe reads through rotation with bounded transient
handles. The null is that substitution, unexpected errors or material resource
regression remains. Rechecking each file's first group adds decode work; measure
that cost. These finite cases are not long-soak, installation or deployment
qualification. No new time, server-memory or storage allowance is allocated.
