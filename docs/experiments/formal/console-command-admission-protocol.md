# Qualification console command admission

Registered 2026-10-12 before the successor smoke or measured cells. This is a
harness prerequisite for the [service/query protocol](release-service-query-protocol.md),
not a change to production admission or the latency/correctness decision rules.
The first `service-opening-r3-main-t10-s1-smoke-01` remains failed: its eighth
source enrollment received HTTP 429 before measurement. Preserve its request,
runtime, source and outer-cgroup receipts.

## Boundary and schedule

The production peer admits eight protected console requests per second with an
eight-request burst ([peer admission](../../../crates/fabric-server/src/peer_admission.rs)).
A [bridge command](../../../tools/qualification/console_bridge.py) performs a
fresh session GET followed by the requested operation. The earlier 0.15-second
logical-command spacing offered about thirteen HTTP requests per second before
benchmark reads or the real UI poll. A successful owner ceremony is not permission
to bypass that peer budget.

Serialize protected bridge command starts at least 0.5 seconds apart under the
existing lock. Their two requests use at most four requests/second, leaving room
for the registered three main visibility probes/second and five-second UI poll.
Keep every session check, current CSRF token, UV refresh and server authority
check. Do not cache authority, omit a session request, retry a refused operation,
discard a response, disable admission or change the server's limits.

The controller is still triggered at measured second 45. Its existing ordered
per-source mutations are subject to this command budget: a 100-source sequence
can span about 50 seconds. Retain each actual command-start, response-completion
and apply timestamp and report the sequence's observed span; a response bounds
publication but is not its exact server commit timestamp. Do not describe the sequence
as 100 simultaneous publications. The existing per-source configuration-apply
deadline remains 30 seconds, including any bridge wait before its request.
Offered telemetry, seed, measurement boundaries, reader probes, UI pause/resume,
polling cadence, custody/query oracles, backlog rule and resource limits stay as
registered. Other reads and an unexpected burst can still receive a refusal;
every such response remains a failed prerequisite or measured request.

## Controls and candidate identity

The retained [counterexample](fixtures/console-command-admission.json) binds the
native failure to frozen `a5dbda7a4fd6465c94f6505b3214cbcbc2130c82`, seed
`0xA11FA001`. [Admission tests](../../../tools/qualification/test_console_admission.py)
independently account for offered requests in integer nanoseconds at the fixed
eight-request rate/burst. They reject the old command-pair schedule and require
room for separately specified visibility/UI traffic. Session/CSRF and explicit
429 propagation checks remain separate properties. This finite model does not
establish arbitrary concurrent traffic or replace the native successor smoke.

Use the same successful exact Debian/RPM build receipts and unchanged frozen
package hashes. Record the harness commit separately from the artifact source;
archive its loaded source before execution. Run the checker controls and a new
disposable smoke through the resource launcher before the full six main and four
query cells. A smoke is never a qualification result, and historical results
cannot be regraded into successor passes.
