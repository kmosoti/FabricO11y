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

## Persistent management clock sampling

Fresh split smoke `split-debian-fedora-smoke-16`, product `108945d` / package
snapshot `1cb0c185`, completed all 19 gate records and both shutdowns. ACK p99
for offered records was 0.871386242 s; all three backlog gates passed, with
edge maximum clearing busy period 1.954342350 s. Exact custody, queries,
resources, configuration, outage and drain passed. All five timing gates still
failed visibility, including censored observations. Keep this failed run intact.

Stopped diagnostic `cadence-evidence-diagnostic-02` exited 0. The added observer
completed only six sweeps: named-query polling intervals reached 26.070271098 s,
while each refreshed query window covered about eleven seconds. Server query
clock brackets had median width 0.877780026 s. Repeated SSH/Python startup for
each raw guest-clock sample is a leading source of observation overhead. This
diagnosis does not establish exact product visibility or regrade the failure.

Keep the constructed packages and all current queries, raw clock fields,
per-response brackets, independent oracles and numeric gates. Through the existing
Guest lifecycle and its management forwarding, allow one owned persistent SSH
clock channel per guest. A request must produce a fresh raw boot/monotonic/wall
sample inside that guest; never derive guest time from host time. Serialize
requests, match sequence numbers, and reject wrong boot, reordered clocks,
malformed/oversized replies, disconnect and timeouts. Bound each channel to
8,192 samples, 4 KiB per reply, 32 MiB memory, zero swap, one-quarter CPU,
eight tasks and 600 seconds. Verify guest unit limits before exposing samples.
These fixture processes remain inside the existing guest/host envelopes.

Close the owned channels on every path. Require original SSH/systemd command
exit status and confirmation that the named guest units have stopped; transport
success alone is insufficient. Preserve failure stderr, startup/stop facts and
sample counts. Exercise correct samples, wrong identity/sequence/order, partial
reply, disconnect, deadline, sample/byte bounds, failed payload and cleanup
failures. Start and stop through the existing guest interface, including the
remote subclass; do not introduce another runner or alter the data outage path.
Repeat affected lifecycle/observer/TLS controls and a fresh full split cell.
This is a prospective harness repair, not new acceptance or publication approval.
