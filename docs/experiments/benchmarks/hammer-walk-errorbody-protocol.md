# Walk live-query failure diagnostic

The first Walk pressure cell retained four HTTP 500 responses for its full-range
absent literal query. Exact source recovery, final query chains and custody
passed; live-query error/completeness gates failed. Preserve that failed result.
The original observer retained the exception representation but not the response
body. The original comparison queue is stopped under its semantic failure rule.

This prospective diagnostic inherits the [pressure protocol](hammer-reference-protocol.md)
and its remaining time allowance. Repeat only Walk, seed 2704101, the same
twenty collectors, source distribution, 60-second 1k/4k/16k stages and 180-second
quiet interval. Keep all server/node cgroups, binary identities, exact oracles
and acceptance gates unchanged. Add opt-in observation of HTTP error status
and at most 64 KiB of body, capped at 256 retained errors. Do not record headers,
tokens or request authentication. Record truncation and overflow explicitly.
Re-raise the original error: an explanatory body does not make a failed query
successful. The unchanged full final/source/ACK checks still run.

Competing explanations are catalog-movement retry exhaustion, a stale file
pathname during journal rotation, a distinct I/O error or worker failure.
The response text and retained query/publication timing distinguish these;
source inspection alone does not identify the observed cause. After identifying
the cause, construct a deterministic counterexample before any production fix.
Do not resume comparative nomination while the failure remains unexplained.

The original failed raw tree is about 4 GiB. To preserve complete failure evidence
within the owner's 100 GB storage ceiling, this diagnostic raises the pressure
round's retained memory-lab evidence allowance from 2 to 8 GiB. Reference and
coordinator caps stay unchanged. A single preservation input is at most 8 GiB,
and each authenticated compressed archive plus its inventory is at most 3 GiB.
Raw trees are removed only after exact member/content verification and unchanged
source inventory comparison. Preservation may take up to 600 seconds per job,
charged to the existing 7,200-second round; no time allocation is added. The
active-work scratch cap stays 8 GiB, with 16 GiB free reserve. These are evidence
storage limits, not changes to the 4 GB Fabric server or remote memory caps.
