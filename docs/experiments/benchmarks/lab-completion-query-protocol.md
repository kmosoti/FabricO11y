Registered by the inline coordinator before execution under [campaign scope](lab-completion-protocol.md). Preparation wording below records the PI handoff; commands have not yet run.

# Q2a allocation completion cell

Pre-run specification for coordinator review under
`docs/experiments/benchmarks/lab-completion-protocol.md`. Status: implemented
private harness; no build, control, test, workload or validator run by Query PI.
This adds a new experiment, preserving `query-plan-protocol.md`, its inconclusive
allocation result, `responsibility_probe.rs`, the old profiling collector and
all existing oracles. Preparation evidence is `completion/query-notes.md`.

Question: which declared first-page query operations allocate the remaining
memory on identical within-layout recovered log input? No production optimization,
plan-default decision, native CPU confirmation, metrics, spans, rate comparison or
qualification follows from this single descriptive allocation screen.

Freeze seed42, 65536 shuffled log bodies of1024 bytes (half repetitive, half
deterministic entropy); tiny128 preflight. Four shapes: empty window[1,2),
selective `bench-0007 `, common `RRRR`, broad. Nonempty shapes cover all fixture
observed timestamps; limit10000 remains unchanged. Layout order tail then Segment;
plan order Scan then Walk; shape order empty/selective/common/broad; each shape
resets History, measures one first call and three warm calls. First means fresh
History caches and buffered OS pages, not cold disk. CPU affinity is inherited
and recorded, fixed for plain/counting variants in the same outer job. No live
ingestion, sealing or retention runs concurrently with query calls.

Build/copy plain then `responsibility-alloc-probe,phase-probe` executables from
NEW `completion_query_probe.rs`. Record binary, source, oracle and collector hashes,
revision, commands and environment. Trial order tiny/plain, tiny/counted,
full/plain, full/counted. Stop dependent trials at any failure. The prior tail
receipt-time difference is explicit: fixture/Segment Groups use fixed receive
times; tail Intake uses actual receipt times. Require exact Batch identities and
bytes, plus fixture/Segment exact ledger equality and tail equality except
`received_ns`; both plans use the exact same recovered ledger per layout.

All64 first/warm calls finish before continuation collection begins. Their
first-page query/serialization measurement logic is copied unchanged. Stream
continuation pages afterward using each recorded token and original query with
fixed newest Group. Exact measured first-page bytes begin each chain. The private
collector grades all64 complete chains through unchanged `query_oracle.check`.
It rejects first-page-only/truncated full chains, changed/missing/duplicate first
or continuation rows and mismatched snapshots. Tiny and full ledgers each retain
their own meaningful controls. First-page byte-association and phase checker
controls inject missing/mislabeled/nested opposite-plan spans before measurement.

The only production edit moves the optional Scan loading span from
`sources_walk` into `sources`; Walk retains its own span. Every counted measured
call must contain exactly one matching loader span. Measured phase records are
flushed before continuation work; continuation phases have a separate file.
Global/nested allocation snapshots are inclusive and cannot be summed.

Metrics: per-call process CPU/wall time, allocator baseline/end/peak/incremental
peak/cumulative requested bytes, measured inclusive phase snapshots and
uninstrumented counterpart; first/warm populations remain separate. Instrumented
timings include allocator/phase-observer overhead and cannot enter native speed
ratios. `measured_queries_complete.vm_hwm_kib` bounds the measured prefix;
final HWM/cgroup peak also include correctness drains and grading. No winner
threshold is selected: report every shape/plan/layout and observer perturbation.

Retain distinct actual answer bytes as gzip content objects only after exact
equality verification on reuse. Preserve original wrapper prefix/suffix, chain
page order, first-page association, full raw hashes/sizes and all exact Batch
framing. Reconstruct every wrapper and page, comparing exact bytes against its
original before deleting trial scratch. No digest substitutes for oracle grading.

Budget: build<=900s total; probes/grading<=1800s total; campaign Query stage<=7200s.
Outer envelope remains16GiB high/20GiB max/no swap. Owned scratch<=8GiB;
data-drive free reserve>=16GiB; combined Query-lab evidence<=256MiB. Every
workload, control and validator runs through the root's resource/coordinator
launcher. Any cap/deadline/oracle/phase failure preserves failed scratch for
coordinator review instead of deleting evidence. Successful fixture scratch is
removed, shared build caches retained; launcher/coordinator provide final receipts.

Coordinator commands (not executed by PI):

```text
python3 tools/resource_group.py -- python3 tools/bench/labs/completion/profile.py --controls
python3 tools/resource_group.py -- python3 tools/bench/labs/completion/profile.py --out docs/experiments/benchmarks/data/lab-completion-run-01/query/profile-01 --build-seconds 900 --run-seconds 1800
```

The combined build/run command needs an outer deadline admitting its2700s maximum
(use the registered deadline extension and root `completion/run_job.py`). Root
must run formatting/compilation, targeted history tests and fast checks separately;
no passing claim exists until their actual exits/receipts are collected.
