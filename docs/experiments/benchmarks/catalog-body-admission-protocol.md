# Prebody HTTP admission schedule

This additive registration fixes the admission schedule required by the
[ownership transition investigation](catalog-transition-ownership-protocol.md)
before its baseline. It changes no prior outcome or resource allocation.

Use the actual Router, Store, Control and History over plaintext loopback HTTP/1,
a two-worker Tokio runtime, 64 KiB journal and 16 KiB rotation. No valid telemetry
is submitted. The fixture owns all sockets, server and writer and closes/joins
them before reporting its desired-behavior assertion. Raw output is a few KiB;
retain failed fixture state and every response under the two MiB driver bound.

Open 16 batch POSTs declaring 1024 body bytes. Each uses Expect: 100-continue;
require interim 100 as evidence that body extraction has actually been polled,
then send exactly one body byte. A seventeenth incomplete request omits Expect
and must receive 503 with Retry-After: 1 before sending the other bytes. A complete
admin query must still succeed. Close all partial sockets; poll complete batch
requests for at most two seconds until the ordinary 401 authentication response
establishes recovered capacity. The supplied admin token is deliberately not a
valid node token; changing authentication is not part of the candidate.

Repeat with two witnessed query bodies and a third excess body. A complete batch
request must still receive its ordinary 401. After closing the partial queries,
poll for a complete query's 200 within two seconds. Excess reads have a 600 ms
deadline and bounded response size. The fixed capacities are conservative
development defaults for this candidate, not a calibrated throughput optimum.

Expected baseline: both excess requests time out/WouldBlock while all four
independent-pool and cancellation-recovery status controls succeed. Rust exits
101. The driver must classify that exact trace, including the 16/2 interim
witnesses, rather than accept an arbitrary compilation or runtime failure.
A modified trace with a 503 overload response must fail that classifier.

After correction, run the identical test and additional deterministic unit
controls showing that a blocking query retains its permit after the async waiter
is cancelled, and releases it after normal completion or panic. Test workers
must be released even when an assertion detects a defect, avoiding deadlocked
runtime teardown. These controls supplement actual HTTP behavior; they are not
independent query-answer oracles or a process-wide memory measurement.

Dispatch `catalog-admission-repro-01`, preparation, 180 seconds, with the existing
three MiB reservation and `transition_ownership.py --mode admission`. All original
frontier, stage, evidence, cgroup and cleanup rules apply. Subsequent fixed and
final jobs are already registered. Linux connection buffers/TLS handshakes,
other admin routes, absolute query-memory bounds and sustained-load latency
remain unmeasured; request counts alone do not establish those claims.
