# Fixed-query page admission prerequisite

Registered 2026-10-12 before its implementation or successor fixed-query trials.
This supplements the [service/query protocol](release-service-query-protocol.md)
without changing its fixture, query list, oracles, latency threshold or UI.

The frozen-package `service-opening-r3-query-segment-s1-01` failed with HTTP 429
inside the 43rd logical query, a source counter history requiring five pages.
Its 42 completed queries produced 66 HTTP pages. The old caller waits 0.1 seconds
between logical queries and sends continuation pages without pacing. Preserve
the failed summary, raw independent source, completed answers, runtime and
confirmed process cleanup. Its HTTP error path did not retain the response body;
do not invent the exact refusal message.

The protected peer budget is unchanged at eight requests/second with an eight
request burst. Pace only fixed-query page request starts at least 250,000,000
monotonic nanoseconds apart. Four page requests/second leave room for the actual
five-second UI poll. Keep the existing 0.1-second inter-query pause, one pending
read, exact credential and every continuation, refusal and error. Do not retry a
429, change the peer budget, bypass authority or cache a response.

Use the existing reader's pagination loop and start its query timer before any
page admission wait. The original p99 <= 2 seconds applies to the complete query,
including admission waits and every page. Record each page's requested, sent and
completed monotonic times and any error status without credentials or response
bodies. The paced attempt timestamp is the caller's dispatch boundary, not a
packet timestamp. Keep the million-row fixture, separately graded 1,000 traces, all seeds,
20 repetitions per kind and phase, restart, independent oracles, resource caps,
CPU placement and cleanup gates unchanged. The six main trials retain their
original three-per-second reader schedule and actual `8484493` evidence.

The [retained counterexample](fixtures/query-page-admission.json) declares an
independent offered-request model using the fixed rate/burst and representative
five-page histories. Inject unpaced page requests: that model and the new checker
must reject them. Check that the full query timer includes the wait and that an
HTTP 429 escapes once without retry. This finite schedule model is not a proof
for arbitrary traffic or a substitute for native fixed-query acceptance.

Run contained checker controls and a disposable smoke before the four fresh
fixed-query successors. Archive the separately frozen harness and unchanged
package inputs. A smoke or a passing subset does not close service/query
qualification. Preserve the first failed cell rather than regrading it.
