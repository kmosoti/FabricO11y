# F1: conditional borrowed-log service confirmation

Status: **draft for preregistration; not registered or executed**. Admit only if
CR2's three plain and three counted component pairs all survive every registered
guard and the independent full-chain checks. CR1 was null: exclude spill reuse.
CQ1 is the common corrected boundary, not a second experimental factor. CQ2/CR3
remain separate; their failed preflights and historical failures remain recorded.
H1: borrowed Segment logs preserve finite native service behavior within 5%.
H0: no useful service effect, a guard miss, or insufficient exercised Segment work.
One baseline/candidate pair per profile is a bounded confirmation, not a statistical
performance claim, sustained acceptance or deployment qualification.

## Freeze and admission

Freeze two uninstrumented server binaries from exactly the same CR2-confirmed
source cut and lockfile: baseline compile flag unset; candidate
`FABRIC_BORROWED_LOG_EXPERIMENT=1`. No shared-catalog/spill selectors. Build the
native node and `examples/server_dump` once and copy identical bytes to both bin
roots. Record exact build argv/env, source/lock hashes, binary SHA-256, features,
compiler and candidate compile-time flag provenance. Existing CR2 freezes contain
query-probe binaries only: they cannot be passed to `signals.py` as server bins.

Root prepares `$FABRIC_F1_BASELINE` and `$FABRIC_F1_CANDIDATE` on owned data-drive
scratch, each containing `fabric-server`, `fabric-node`, `examples/server_dump`.
The contained preparation command is a separately captured script implementing:
`cargo build --offline --locked --release -p fabric_o11y --bin fabric-node` and
`cargo build --offline --locked --release -p fabric-server --bin fabric-server --example server_dump`,
first with the borrowed flag unset, then rebuild
`cargo build --offline --locked --release -p fabric-server --bin fabric-server`
with the flag set and preserve its candidate immediately. Both use the same
production features; omit allocator/phase features. Capture cargo receipts and
hashes; do not rely on setting a runtime flag for `option_env!`.

Prerequisites: registered protocol; source frozen; CR2 nomination; intact SDK
1.38.0 venv and dependency receipt; four usable CPU affinities; verified delegated
cgroup controllers; private TLS/ports; data-drive reserve at least16GiB;
8GiB scratch/job; combined existing memory evidence plus forecast below its
registered512MiB allocation AND signals.py's still-enforced256MiB memory-directory
cap. No disk-cap weakening. Reserve **3000 seconds** of remaining aggregate budget
(plus600s preparation and2400s signals-stage admission) before starting: freeze<=600s, four cells<=450s each,
audit<=120s and480s margin. Re-read current receipts before admission; earlier
remaining-time observations are not authorization. Stop before a cell if its
whole450s allocation cannot fit. No pressure/lifecycle or qualification run.

## Four serial cells

Use the existing signals harness unchanged, mixed signals,180s offers, opt-in
Walk, verified nested dev/small limits. Baseline then candidate for development;
reverse candidate/baseline for small to expose the order, without treating it as
replicated randomization. Substitute the frozen absolute bin directory in each:

```sh
python3 tools/resource_group.py --delegate -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-f1-dev-baseline-01 --lab memory --stage signals --seconds 450 -- python3 -B tools/bench/labs/completion/signals.py --id catalog-f1-dev-baseline-01 --profile development --signals mixed --duration 180 --limits --query-plan walk --bin-dir "$FABRIC_F1_BASELINE"
python3 tools/resource_group.py --delegate -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-f1-dev-candidate-01 --lab memory --stage signals --seconds 450 -- python3 -B tools/bench/labs/completion/signals.py --id catalog-f1-dev-candidate-01 --profile development --signals mixed --duration 180 --limits --query-plan walk --bin-dir "$FABRIC_F1_CANDIDATE"
python3 tools/resource_group.py --delegate -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-f1-small-candidate-01 --lab memory --stage signals --seconds 450 -- python3 -B tools/bench/labs/completion/signals.py --id catalog-f1-small-candidate-01 --profile small --signals mixed --duration 180 --limits --query-plan walk --bin-dir "$FABRIC_F1_CANDIDATE"
python3 tools/resource_group.py --delegate -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-f1-small-baseline-01 --lab memory --stage signals --seconds 450 -- python3 -B tools/bench/labs/completion/signals.py --id catalog-f1-small-baseline-01 --profile small --signals mixed --duration 180 --limits --query-plan walk --bin-dir "$FABRIC_F1_BASELINE"
```

Development: one node,10/30/10logs/s over60/60/60s, SDK10parent/child pairs/s.
Small:20nodes,1000/3000/1000aggregate logs/s, SDK50pairs/s. Seed2703163393,
900-byte alternating entropy/repetition bodies,15s host metrics,64MiB journals,
one seal worker; server restart halfway with2s stopped interval is preserved.
Dev server/node maxima512/128MiB; small3072/256MiB; quotas2/1CPU; host aggregate
640/3328MiB. Outer20GiB/no swap unchanged. Record actual cgroup enforcement.

## Evidence and guards

Require every existing summary gate: exact all-source log hashes, SDK raw accepted
trace bytes/source identities/flush, ACK-to-recovered Batch SHA custody, metrics,
unchanged independent logs/metrics/spans full-chain oracle, valid clocks, no OOM.
Missing/duplicate/altered controls remain unchanged. A correctness miss stops
nomination and preserves its failed run; query errors during the intentional
restart remain visible, while errors outside that bracket fail this screen.

Archive environment/summary/rates, sources/events/lifecycle/query samples, complete
pages/verdicts, payloads/recovered hashes, SDK attempts, span-clocks, application
cgroups, coordinator recursive process RSS/HWM/CPU/IO samples and cleanup receipts.
Use integer monotonic phase times; exclude startup/drain/restart from phase CPU
and latency comparisons. Sum server CPU across both restart PIDs; distinguish
server/node RSS from whole-job cgroup peak. Per profile require candidate/baseline
<=1.05 for matched-phase server CPU, sampled server RSS peak, ACK-attempt p99,
SDK successful local-response p99 and successful query p99. No undefined/zero
baseline ratio passes. Keep sparse p99 counts visible; no population inference.
Native collection accepted batches/s and unique ACKs/s must be>=95% baseline;
missing recovery keys must be zero. Record encoded byte rates separately because
host metrics/timestamp batching vary. Successful SDK acceptance must cover all
offered spans as existing gates require; source offers and Spool acceptance are
separate counters. Require final drain/recovery exactness and candidate maximum
sampled spool backlog<=1.05baseline; report phase/backlog slopes descriptively.

`span-clocks` records local Spool response, server receipt, observed ACK and first
query appearance. Missing appearance stays null; periodic10s/20s-window samples
are coarse visibility observations, not exact latency. Require candidate observed
fraction>=baseline and p99 nonnegative local-response-to-observation delay<=
baseline+10s; report missing and negative/order-ambiguous timestamps separately.

Development may never naturally publish a64MiB Segment: it is a native regression
control, not evidence of the borrowed path's benefit. Small must observe natural
publication plus successful log queries afterwards, else Segment service effect
is inconclusive. Periodic logs use broad first100 rows; final selected logs use
node00/prefix. There is no native H256/rare-text consumer or measured full-chain
latency population here. Source SDK IDs/host metric values vary across processes;
match configuration and deterministic log workload, not raw Batch byte equality
between trials. An independent result audit/ratio script still needs preparation
and registration before any decision; harness exit0 alone cannot enforce these
additional comparative guards. No service benefit is claimed in advance.
