# Spindle cadence and visibility observations

## Retained failure and diagnostic boundary

Split smoke `split-debian-fedora-smoke-15` uses product `aaca52d`, packaged
snapshot `c0cc668` and harness `5365eaf`. Its complete 19-gate record has exact
custody, all query phases and independent offer counts passing, but offered-edge
ACK p99 upper bound 1.016655693 s exceeds 1 s. The five visibility p99 upper
bounds span 6.591375325–9.413178984 s, above 5 s. The edge's longest source-debt
busy period is 25.089556484 s, exceeding the registered five-second clearing
bound. Both host shutdowns were confirmed. No release acceptance is credited.

Use a contained read-only diagnostic on the original stopped evidence before
choosing a fix. Separate measured ACK distributions by record kind and compare
creation-to-ACK with actual successful-attempt time. Inventory visibility sweep
clock widths, per-node polling intervals and the age of each query's declared
upper bound at completion. Retain all rows, original gate outcomes and hashes.
Diagnostic statistics cannot turn an upper bound into an exact visibility time,
produce inferred timestamps or promote the failed run.

Leading hypotheses: idle trace polling and one-second log polling contribute
source waiting, while grouped clock brackets and aging per-name query windows
contribute visibility uncertainty. Compare these against actual transport time
and preserved query/clock witnesses. Keep all independent oracles and original
thresholds. Register any repair separately before new execution; product changes
require successor packages, while observation-only repairs require fresh affected
controls and cells.

## Prospective product cadence repair

The stopped diagnostic `cadence-evidence-diagnostic-01` exited 0. All 48 measured
ACK samples above 1 s were offered file logs (240 total; log-only p99 upper bound
1.22570409 s). Measured spans had p99 0.631171364 s and none above 1 s. Successful
attempt p99 across the retained edge population was 0.178025444 s; this aggregate
also includes recovery and is not a measured-phase transport comparison.

Keep native metrics on their configured interval. Change idle log polling from
one second to 250 ms, and cap the requested idle wait at 10 ms when the existing
loopback trace listener is enabled; retain 100 ms for other idle waits. Clamp
both by the existing remaining deadline, including zero. Do not change delivery
slice deadlines, commit/ACK ordering, backoff, rate limits, payloads, APIs or
resource caps. These are requested scheduling intervals, not real-time bounds.
Require deterministic deadline/zero/positive-wait controls and rejection of the
old 100 ms trace-idle choice. New binaries require a successor candidate and all
affected gates. End-to-end latency, CPU and RSS still require full execution.

## Prospective per-response visibility observation

The diagnostic retained bounded-observer sweep widths up to 6.528873589 s for
edge and 6.960957502 s for server, with start-to-start gaps up to 9.316169508 s.
These are conservative observation bounds, not exact product visibility times.
Keep every original query, named query schema and independent oracle. Keep one
original reader and one observer in the existing two query slots.

The existing HTTPS fixture caller may reuse at most two authenticated, strictly
verified TLS connections, one per calling thread. Keep authentication on every
request, bounded response bytes, existing timeouts and no retry of an uncertain
request. Close all owned connections on success and failure. Require real
trusted-CA/hostname positives and wrong-CA/wrong-SAN refusal, connection reuse,
request-authentication, HTTP-error body and disconnect controls.

For the added observer, retain the raw guest-clock bracket immediately after
each complete answer. The preceding answer's raw after-clock (or the fresh
sweep-start clock) is a valid before-clock for the next query. Refresh each
named query's existing rolling wall-time window from that before-clock. Preserve
all raw witnesses; do not infer a guest clock from host time, narrow an old
witness or regrade the failed run as accepted. Delaying a later name must not
widen the first name's already completed clock bracket. Keep actual eight-request
admission overload refusal and all prior observer failures. Repeat the affected
controls and a full fresh cell with unchanged offer counts, outage, deadlines,
backlog/latency gates and host budgets.
