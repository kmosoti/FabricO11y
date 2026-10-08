# TLS startup failure and journal ownership

Registered before the reproduction or correction. End-to-end reliability includes
recovering from configuration errors without leaking a live storage owner.

Hypothesis: `fabric_server::serve` starts commit/sealer threads before awaiting
TLS certificate loading. Missing/invalid TLS files return early without setting
the sealer stop flag or joining its workers; the sealer's Intake keeps the commit
writer and journal lock alive. Null: after the expected TLS error, the same
process can reopen the journal within the bounded probe.

Add two desired-behavior regressions in `tests/startup.rs`. Use a fresh owned
directory and generated EC certificate/key, a valid admin token, 1 MiB journal
and 64 KiB rotation. Establish a successful Store open/drop before startup.
Then either remove the certificate or replace the key with invalid PEM. Call
the real `serve` with a five-second timeout; require an actual error, not a
timeout or successful serve. Attempt Store reopen for at most two seconds,
recording exact error kinds/messages and results before asserting success.
Execute serially. Fixtures and traces survive assertions.

Run both unchanged-production regressions first. Expected Rust result is exit
101 with both TLS errors followed only by failed journal reopen attempts. The
driver may exit 0 only after establishing this exact counterexample, recording
the Rust checks as failed. A mutated trace containing a successful reopen must
be rejected by this classifier. Archive all small failed fixture files, with
manifest and exact byte readback, before cleanup. An unexpected result stops
the correction and requires investigation.

If reproduced, validate TLS before acquiring Store/worker ownership. Keep valid
startup, authenticated delivery, journal sync and shutdown semantics unchanged.
Re-run the two unchanged tests, existing TLS delivery integration, and final fast
and manual documentation profiles. A constructor-order correction makes no claim
about cancellation of an already serving future, arbitrary worker-spawn failures,
physical power loss or deployment qualification.

Root dispatches serially through `resource_group.py` and `completion/run_job.py`:

| Job | Stage | Deadline | Reserve after coordinator snapshots | Driver cap |
| --- | --- | ---: | ---: | ---: |
| `catalog-startup-repro-01` | preparation | 180 s | 1 MiB | 512 KiB |
| `catalog-native-lifecycle-01` | preparation | 360 s | 10 MiB | 8 MiB |
| `catalog-native-lifecycle-checks-01` | verification | 600 s | 1 MiB | 512 KiB |

The middle job may include the corrected startup controls and a separately
registered native lifecycle fixture; this document does not yet define that
fixture or authorize its execution. Final verification includes exact Bun archive
reference cleanup as in the prior round. Sources, hashes, commands and real exits
must be retained. Use the mounted data drive, 20 GiB maximum, 16 GiB high, no swap,
and the 30-minute outer deadline. No remote work; all existing evidence/frontier/
stage limits remain. Approximately 2,153 frontier seconds remain before this round.
