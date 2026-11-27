---
id: R-0299
title: N02 resource and benchmark declaration prototype
type: research
status: preliminary
date: 2026-11-27
added_by: agent
human_ratified: false
---

# R-0299 · N02 resource and benchmark declaration prototype

[#299](https://github.com/kmosoti/FabricO11y/issues/299) is the manifest-first
slice of [#234](https://github.com/kmosoti/FabricO11y/issues/234). The
[resource declaration](../../xtask/tests/fixtures/n02/resource-profile.toml)
and [benchmark declaration](../../xtask/tests/fixtures/n02/benchmark-profile.toml)
are test fixtures, not a normative schema, deployed settings, or a CI gate.

- **Resource declaration:** keep the collector process RSS target (<100 MiB),
  shedding ceiling (200 MiB), and FD target (<64) distinct from the shared
  systemd slice's 256 MiB pressure threshold and 512 MiB hard memory limit.
  Slice memory accounts for all members and includes cache; adding process RSS
  does not reconstruct it. The Spindle transport example distinguishes a
  per-connection envelope from the process aggregate: eight fixture connections
  times `(65,536 request + 73,728 reply + 4 header)` bytes. That ceiling covers
  only the named wire buffers—not sockets, worker state, or the process RSS.
  The observation JSON depth limit is a parser bound, not a memory estimate.
- **Benchmark declaration:** use the pinned `collection-resources` full-width
  scenario (eight slots, 4,096 encoded bytes per slot, four producers). The
  predeclared correctness thresholds are at most 32,768 bytes of reserved
  SDK payload and zero lost accepted records. Failed or refused attempts are
  reported, not counted as lost accepted work or removed from the record.
  Collector targets are not applied to the Spindle process. Other performance
  metrics are report-only here; no slowdown/speedup threshold is invented.
- **Baseline and comparable runs:** the source and Rust compiler revisions
  identify the intended release build, not a completed run. Bind each future
  execution to its actual source and acceptance receipt, Cargo lock, executable,
  fixture and resource profile, and the machine profile. Compare on one matched
  host including topology, kernel, affinity, and cache conditions. Predeclare
  one warmup and five measured repeats. Keep raw samples, variance, setup,
  outage, recovery, failures/refusals, and unavailable-counter diagnostics in
  a separate result artifact. Unknown or unavailable values are not zeros.

The existing [collection resource program](../../xtask/src/bin/collection-resources.rs)
checks outage saturation, retained reservations, slow connections, and
post-recovery custody. Its point-in-time RSS readings are not a peak RSS or
cgroup memory measurement. This prototype adds only [targeted tests](../../xtask/tests/n02_manifests.rs)
for declared caps and parser refusal, scope ordering, aggregate arithmetic,
workload drift, and rejection of result fields inside declarations. These checks
do not run that benchmark program or establish actual daemon/slice compliance.

Still unqualified: RV1–RV6 inputs and budgets, auth-cache, DataRef and session
budgets, end-to-end queue/index/live-ring/maintenance coverage, and transport
across multiple processes. A missing entry is not a zero or unlimited cap.
Extending `doctor` or emitting evidence through [#298](https://github.com/kmosoti/FabricO11y/issues/298)
is separate. [#324](https://github.com/kmosoti/FabricO11y/issues/324) owns the
representative native-suite scheduling comparison; its historical timings do
not predict a speedup here. This note contains no measured results.

## Executable declaration checks

On 2026-11-27, the test-only schema and refusal checks passed against the
workspace based on `develop` source `7de9e1918802f51ca108b1e9fd54005236982390`
using Rust `1.100.0 (0ef1bc386 2026-11-09)`. The resource and benchmark
fixtures had SHA-256 digests `52061b1466adb49c217f8de032c56b8af7b42a1d705028d993165c86f6bd2e55`
and `2d71747fdd2866b88edb6fdfff263f45c4098a0503ba4524058fdcfd849d2079`,
respectively. `cargo test -p xtask --test n02_manifests -- --nocapture` passed
all five tests, including parser depth/frame refusal, scope inversion, checked
aggregate arithmetic, workload budget drift, and rejection of observations in a
declaration. This test result qualifies the declarations and those boundaries;
it is not a run of `collection-resources`, a performance measurement, or a
runtime process/slice compliance result.
