# Remaining development/small campaign execution

Registered before new workloads. Owner scope: “Feel free to complete all the
experiments in the plan. Collect the data.” This authorizes the remaining cells
of [the campaign](dev-small-lab-plan.md), including its owned fault experiments,
full-duration soak/outage/lifecycle, deployment-limit experiments and disposable
supported installation. It authorizes no release, tag, merge or production default
change. Existing protocols, failed outcomes and independent oracles stay intact.

## Admission and budgets

Run workloads serially. Every project process tree remains inside
`tools/resource_group.py`, with 16 GiB memory high, 20 GiB maximum and zero swap.
Add an explicit finite deadline argument (default remains 1,800 seconds); admitted
long jobs may request up to 16,000 seconds. This is the owner-authorized deadline
extension for these named experiments, not removal of containment. Lower nested
application cgroups apply in C4; host-appropriate containment applies remotely.
Never replace cgroups with address-space or RSS limits.

Finite campaign ceiling: 86,400 measured command seconds, including builds,
validators and cleanup. Stage admission ceilings: preparation 3,600; capacity
7,200; query 7,200; recovery 7,200; signals 5,400; deployment limits 5,400; soak
7,500; outage 9,600; fresh lifecycle 15,500; installation 7,200; verification 3,600.
Unused allocations do not justify unrelated experiments. Admit full trials only
when their remaining estimate fits; preserve interrupted/failed outcomes.
A failed prerequisite stops its dependent cells, not independent lab work.

Data-drive caches and owned scratch only: `/run/media/kmosoti/data/FabricO11y`.
Per-job scratch ceiling 8 GiB, free-space reserve 16 GiB. Frozen qualification
runners retain their stricter 1/5 GiB live-data and 1 MiB evidence limits. New
campaign retained evidence ceiling 256 MiB per lab, 2 GiB aggregate; store exact
counterexamples and compact reproducible ledgers rather than redundant stores.
Resource sampling can miss short transient peaks; cgroup peaks/events remain
part of receipts. Never discard a failure to report a pass.

Evidence root: `data/lab-completion-run-01/`. Three standing responsibilities:
capacity/lifecycle PI, query/availability PI, inline recovery/operations PI and
coordinator. Estimated delegation envelope 36,000 tokens: 12,000 initial inspection,
16,000 implementation, 8,000 reserve. Actual telemetry is unavailable; this is a
finite dispatch estimate, not measured billing. A Luna assistant was requested but
runtime thread admission rejected it; no replacement agent is created merely to
fill roles. Root executes all workloads; agents prepare isolated fixtures.

## Remaining experiments and invariants

- **C1-6:** one real development node, H256, original Scan mix and 10/30/10 logs/s,
  unchanged 60/60/60 schedule plus settling/drain and original independent guards.
  Fresh campaign paths and budget; historical source/config hashes are preserved.
- **Q2a allocation:** separately register corrected full-pagination collection
  outside first-page timing; unchanged independent oracle, exact measured first
  pages, plain/counted fixtures and meaningful rejection controls. Correct only
  optional mislabeled source spans. Native three-pair results remain historical.
- **Q1/Q2b:** retain full-chain exactness through seal/retention, quiet sources,
  explicit gaps, age/byte eviction, expired tokens, raw corruption and optional
  index fallback. Use owned deterministic small fixtures; expected retained ledger
  distinguishes legitimate eviction from missing custody. Register new fixture
  details before execution; existing independent oracle is not weakened.
- **C2:** faithful registered four shapes at 64 MiB, steady 16/32/128/256 MiB,
  three fresh processes each, exactness/determinism/pruning and registered heap
  bounds. First isolate the earlier logical-versus-physical row-group checker
  interpretation; a diagnostic reconstructed fixture cannot pass acceptance.
- **R1:** stage-specific owned IO faults and actual owned-child kills, independent
  recovery grading, no shared-drive filling or host-service kills. Publication,
  raw/filter/table/manifest and checkpoint/reclaim expectations remain distinct.
- **R2a:** pinned representative SDK trace-only then mixed native logs/host metrics/
  traces, declared exported bytes and batching; separate SDK/local-Spool response,
  server ACK, projected semantics and raw payload fidelity. Register before load.
- **C4:** verified nested development server512MiB/node128MiB; small server3072MiB/
  node256MiB with installed aggregate semantics per modeled host. Outer20GiB holds
  all clients/graders. Record CPU/task settings, ordinary/burst/drain then explicit
  pressure and recovery with expected gaps. Failed limits remain failed candidates.
- **C3:** unchanged soak5,460s using frozen runner7200s (outer7500s); three unchanged
  1800s-outage seeds with runner3000s (outer3100s). Fresh development lifecycle
  stays at10logs/s and64MiB journals through at least two natural rotations; use
  observed encoded rate to admit a complete run, up to15000s outer. No accelerated
  lifecycle is represented as the default duty cycle.
- **R2b:** actual supported Debian-family/systemd installation in a disposable,
  contained environment, including registered negative controls and operator data
  preservation. Inspect host/container compatibility first. A too-small remote
  or unavailable running systemd environment is recorded as unavailable, not pass.
- **Closure:** required fast/extended checks, exact exit receipts, resource and
  cleanup observations, and a per-profile/signal evidence-to-claim report. Scope
  here does not waive environment prerequisites or turn local measurements into
  target-host qualification.

Freeze binaries/configs for each declared comparison. Preserve source/diff,
commands, clocks, hashes and raw populations. New cell specifications are separate
pre-run addenda; no retrospective changes to a measured decision rule. End with
all completed results, counterexamples and unavailable prerequisites explicitly
recorded, not a claim that every hypothesis succeeded.
