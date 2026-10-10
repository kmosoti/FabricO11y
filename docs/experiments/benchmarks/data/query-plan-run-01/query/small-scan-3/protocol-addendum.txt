# Development/small lab screen: low-rate probe correction

Addendum to the [registered screen](dev-small-labs-screen-protocol.md), written
before admission of C1-5. All workloads, guards, metric definitions, budgets and
decision rules remain those of the original protocol. No original result is
rewritten.

## Counterexample and correction

Pre-run source review found that the ordinary visibility selector assumes a
nonempty write on `tick % 50 == 0`. C1-5's 100 logs/s across 20 nodes offers
half a log per node per 100 ms tick. Normal/recovery writes occur on odd ticks,
so the old selector misses those phases and selects only 12 burst targets.
This is a probe-selection defect, not a product availability failure.

Ordinary probes select the first actual eligible source write in each five-second
tick bucket. There is still one target per bucket, 36 in total, with original
source timestamps and 250 ms polling. C1-5's normal/recovery selection is 100 ms
after the bucket boundary. High-rate cells and fresh development have a write on
every tick and keep their original selected ticks. Q1's jittered branch is
unchanged. Preserve the old 12-target counterexample and test the repaired 36
targets before launching the first affected cell.

Only private observer/fixture code changes. Production code, independent answer
oracle, acceptance guards and prior evidence remain fixed. New cell environments
record this addendum and the updated source hashes. C1-5 was not previously run;
its first admitted run supplies fresh evidence under the corrected selector.
