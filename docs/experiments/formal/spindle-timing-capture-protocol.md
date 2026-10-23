# Bounded Spindle timing capture

## Counterexample and scope

`split-debian-fedora-smoke-14`, using product source `1eaf9ad` and packaged
snapshot `531e67c`, completed its two-worker producer, outage and drain, then
refused timing reconstruction on `timing dropped_events=790`. Original edge
stdout SHA-256 is
`bc58c49e121e665b30bc042343e05efccddbc779c7d12aa49b095700ccba0d05`.
The earlier eight-worker run reported four dropped events; one worker failed
its original 320-second deadline. None is acceptance evidence.

The runtime stores opt-in timing events in a bounded queue and the CLI drains
it between turns. A recovery turn may create more than the existing 256 events
before that drain. Complete registered timing evidence is therefore blocked.
Do not suppress the loss marker, infer missing timestamps or grade a subset.

## Prospective repair and decision rule

Raise only the opt-in timing queue capacity to 16,384 events. Keep lazy
allocation, FIFO order, exact drop accounting above the cap, and resetting the
reported drop count when the caller takes the events. The normal collector,
Spool/ACK order, HTTP behavior, wire formats and public signatures stay fixed.
The event array must occupy at most 2 MiB at the cap; taking a copied event vector
can temporarily require a second array. This is a buffer bound, not an RSS proof.
The registered 64 MiB edge RSS limit and every host/service resource cap remain.

Retain a deterministic real-Spool 129-commit counterexample: two events per
commit exceed the old 256-event queue. Require all 258 ordered events and zero
loss without changing the durable prefix. Exercise the full 1950-export,
four-stage finite event population separately. Fill beyond 16,384 and require
the exact original prefix and a nonzero exact drop count, followed by an empty
second take. Disabled timing must remain empty.

Run affected Rust tests, fast, security/extended, dependency and documentation
checks. A product change requires a successor package candidate, fresh hashes,
reproducibility/inspection, updated exact admissions in a separate policy commit,
and all affected release gates. Repeat a full disposable split smoke, retaining
two trace workers and the unchanged offer clock, full outage, deadlines, oracles
and numerical gates. No acceptance or losslessness claim follows before that run;
a finite queue can still overflow on other workloads.
