# Installation-wide fixed-query oracle inputs

Registered 2026-10-12 before the successor corpus adapter. This corrects only
the fixed-query input adapter in the [release protocol](release-service-query-protocol.md).
[ADR-0029](../../decisions/ADR-0029-use-three-fixed-roles-for-the-first-release.md)
and the product contract give readers all telemetry. The older projection to
fixture enrollments describes the former scoped role and cannot grade this role.
Its results remain attached to their actual harness revisions.

The first paced disposable smoke, `service-opening-r3-query-segment-s1-smoke-02`,
completed all ten queries without refusal. Four fleet/text verdicts failed
retained-window/freshness checks because the adapter omitted the mandatory
`fabric-server-self` source. Preserve this failed run. Its independent Spool
contains two contiguous source Batches with no gaps; the source payloads and
native timing logs are retained privately. Do not remove metadata checks.

Keep every original fixture Batch as an independent source. For the companion,
read the stopped, complete Spool prefix and validate custody with the existing
checker. Join recovered identities and bytes against those original Spool bytes;
obtain only receive timestamps from recovery. Require the reserved source label,
one stream, contiguous recovered prefix and no unexplained additional source.
Never obtain expected telemetry or a source payload from query answers.

Companion collection continues during the queries and restart. A first-page
snapshot may include a concurrently committed source. Record that page's actual
caller attempt and completed monotonic times. The independent native same-boot
timing log bounds its possible prefix: include at least every sequence ACKed by
the attempt, and include no sequence accepted after completion. Enumerate only
prefixes within those bounds from the independently byte-verified receive join.
Cap the ambiguous interval at 32 prefixes and fail closed on missing timing,
dropped timing, invalid ordering, inconsistent identity or unavailable sources.
Each candidate corpus contains the entire ACKed fixture and that companion
prefix. Use the unchanged signal/node projection and unchanged query oracle on
each candidate; accept only an oracle-exact complete answer for one independently
admitted snapshot. Record the bounds, selected prefix and original verdict.
Do not modify returned metadata or invent a narrower reader credential.

This remains a zero-gap adapter: any gap in the admitted fixture or companion
source, or any returned gap, fails admission. It does not qualify general gap
redaction. Preserve the full original custody witnesses, exact payloads, hashes,
query list, seeds, repetitions, UI, restart, page pacing and all existing gates.
Every page wait remains inside the original p99 <= 2-second complete-query timer.
The six main trials and their grader are unchanged.

Before native successors, reject representative defects: recovery-byte mismatch,
unknown label, missing recovered prefix, omission of an ACKed companion, inclusion
of a source accepted after the response, and forged freshness/retention. Require
exact answers for a fixed prefix and a genuinely concurrent prefix, as well as
unchanged one-source filtering. Then run a fresh disposable smoke followed by the
four full fixed-query cells; retain both predecessor failures.
