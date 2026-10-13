# Registered separate simulator exposure witness

Prospective registration, 2026-10-13. Coordination
[C-081](https://github.com/kmosoti/FabricO11y/issues/59#issuecomment-6138310799)
admits the separately frozen qualification helper and its controls, **not a
build, native trial, smoke or full soak**. The source/transition proposal is
[wiki revision 875f7f2](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-soak-simulator-source-transition-amendment-proposal-01),
SHA-256 `c36e7068c2d46663d755598d2222b09b8e8068a865d1387f403b8e5843b6bfc4`.
This file registers helper identity and lifetime only;
the separately registered R3 phase amendment belongs to the soak harness.
No result is recorded here.

## Source question and fixed scope

Frozen candidate `8fb6f413179b094288316e25845c137440ba23fa` parses no live begin
or hold-through-boundary option in
[`examples/spindle_sim.rs`](../../../examples/spindle_sim.rs). Its worker exits
once the final due offer has been acknowledged; its begin timestamp appears in
the summary only after joins. These are source observations, not measurements
of a two-hour run. Keeping external probes alive after simulator exit does not
satisfy native lifetime or supply a native pre-offer witness.
The unchanged Rust source SHA-256 at registration is
`2b2828fed73cfe654fadc6739e31c51b9004a42a5174f51be2cb08b161385264`.

Only `examples/spindle_sim.rs` may change **Rust compilation inputs**, from the
exact candidate base. Preserve workers, identities, seeds, due-offer scheduling,
rates, bodies, burst behavior, payload/wire bytes, retries, ACK interpretation
and existing drain/failure semantics. No dependency, core or product change is
admitted. The optional `--begin-witness PATH` mode preserves old callers when
absent; it must not silently turn on for another workload. Full R3 uses 7,200
seconds, 100 identities/workers and `0xA11FA001`; short native controls need a
separately frozen command and do not establish full exposure.

## Two independent source authorities

Canonical packages, package-source tuple, three-helper manifest and helper
bytes remain unchanged. The original
[`cross_family/inputs.py`](../../../tools/qualification/cross_family/inputs.py)
must still require that exact package source and all three original helper
hashes. `server_dump` and `spool_dump` remain the graders; a new simulator must
not replace canonical `spindle_sim` or rewrite its manifest.

The additional owner-admitted helper record binds candidate base, clean helper
commit/tree, complete source manifest/bundle, changed helper-source hash,
binary hash and actual build receipt hash. Compare the **complete compilation
input path sets and bytes** against canonical source: only the helper path
may differ, and it must actually differ. Cargo manifests/lock/config, all other
Rust/protobuf/build inputs and complete vendored/transitive sources remain
equal. Added/deleted inputs, symlinks, uncensused generated input or an altered
build configuration block admission. Documentation and prospective Python
controls do not excuse a Rust compilation difference. `prepare_soak.source_paths`
is a source-census precedent, not proof that an incomplete hand-written list
is complete. Freeze dependency/toolchain/configuration and relevant environment
inputs too; refuse a dirty or unbound build source.

The build record binds actual command, Rust/toolchain/target/profile/features/
flags, source tuple, output hash, exits, cgroup/resource observations and
confirmed descendant stop. Loaded helper identity must agree with that record
and the independently admitted expected tuple. Hashes in an untrusted witness
or mutually agreeing receipts are not independent source authority. Canonical
admission runs first; the opt-in helper admission is additional, never a default
exception to it.

## Exclusive native begin and conservative lifetime

The proposed native path opens/hashes the kernel's loaded `/proc/self/exe`,
records held-descriptor device/inode plus PID, process start ticks and boot ID,
and checks identity while reading. Independent controller observations bind
the actual executable and original process identity, not just its pathname.
Linux documents the executable reference in
[`proc_pid_exe(5)`](https://man7.org/linux/man-pages/man5/proc_pid_exe.5.html).
Expected binary/source/build hashes come from the separate admitted record;
the native program does not invent its source commit from an argument.

Bracket the existing `Instant`/Unix begin with actual `CLOCK_MONOTONIC` reads
from the same process. Preserve the original `began_unix_ns` used by events and
summary. Store integer `mono_before_ns`, `mono_after_ns` and the Unix anchor;
refuse reversed/negative brackets or width over **1,000,000 ns**. Use the upper
monotonic bracket endpoint for a conservative completion boundary. The bound
is prospective; no host observation yet establishes that it is attainable.
No wall-clock offset or host clock change is allowed. The existing pinned libc
dependency is available; no new dependency is needed. Linux's
[`clock_gettime(3)`](https://man7.org/linux/man-pages/man3/clock_gettime.3.html)
defines the distinct realtime/monotonic domains. A numeric timestamp alone
does not establish that the proper clock was called.

Write a complete private record to an exclusively created owned staging file,
sync it, then publish to the fresh final witness path without overwrite. A
same-filesystem hard-link publication is the proposed no-replace operation;
[`link(2)`](https://man7.org/linux/man-pages/man2/link.2.html) refuses an existing
destination. Confirm canonical owned parent paths, no links/mounts, regular
single-link final file, mode 0400 and unchanged held-descriptor bytes; remove
only the staging name this invocation actually created after publication.
Retain partial/uncertain evidence. Publication must complete before worker
offers; failure, an existing destination or a changed witness must produce no
successful trial. Native/file-operation controls must establish exclusivity,
atomic full-record visibility and pre-offer ordering; self-asserted booleans in
the witness cannot establish them.

In witness mode, successful completion must wait until both the original
`Instant` elapsed interval reaches the requested duration and a native
monotonic end read reaches `mono_after_ns + duration_ns`. This conservatively
covers anchor uncertainty; it neither shortens exposure nor invents an exact
end. Record original elapsed nanoseconds, actual monotonic end, begin hash,
parameters and process identity in the end summary. Full-drain failures and
remaining undelivered work still refuse success. Native wait/transition time
remains inside the separately bounded exposure transition allowance; no phase
or outer deadline grows. Old callers keep their original behavior.

## Independent literal controls and admission mechanism

[`test_simulator_exposure.py`](../../../tools/qualification/test_simulator_exposure.py)
contains hand-written two-second/two-identity receipt records and a deliberately
small compilation map. Those records are not real receipts or a complete
native-source census. They exercise two prospective pure Python interfaces:
`validate_source_build(source, build, canonical, expected)` and
`validate_native(begin, summary, offers, loaded, expected)`. These read no clock
and authenticate no owner. The test-only reference encodes the registration;
it is not a fallback implementation or an oracle derived from Rust.

`source/build/canonical` consume caller-read manifests and actual build file
hashes; `expected` is separately pinned. `begin` wraps caller-read witness bytes
as `{'sha256': actual_hash, 'value': parsed}`. `summary` is the original parsed
native summary, and `offers` includes original created-event evidence. `loaded`
joins independent process/binary/source/build observations and stop/exit plus
publication-control observations. Its flags are an **integration obligation**,
not evidence when fabricated by the helper. Parameter fields and identity names
are fixed by the literal records. Before grading, independently bind every raw
file hash and the exact observer/unit/cgroup/command/stop receipt.

The literal controls require:

- exact conservative boundary success and refusal one nanosecond early in
  original elapsed time or the monotonic end;
- absent, changed, nonexclusive, non-atomic, late or nonregular witness refusal;
- reversed, oversized, negative or incorrectly typed clock brackets;
- summary/witness begin/parameter/process disagreement, changed loaded
  executable device/inode/hash, process start/boot, helper source or build hash;
- every identity's original created sequence for every scheduled second,
  without a missing worker/identity, duplicate, extra sequence or early stamp;
- wrong source/bundle/build/toolchain/stop, any canonical helper substitution
  and every added/deleted/changed non-helper compilation input.

Created-event coverage supplements the unchanged source/rate/delivery oracles;
it does not establish body correctness or actual thread count on its own. The
canonical-source control pins the original guard, which must remain unchanged
after native implementation too. The changed helper is admitted by a separate
source record while its original bytes remain pinned as the canonical input.

Initial source execution is expected to report one named missing
`simulator_exposure.py` availability failure, eight literal/static positives, and seven explicit
mechanism-dependent skips. That red is **mechanism absence**, not a demonstrated
Rust timing or product defect. After root records it, a separate production
correction may supply the bounded checker; unchanged controls must then run.
No literal record, source grep, shortened timer or Python check proves that the
compiled simulator held its native lifetime.

## Native prerequisites and execution limits

Before any compile, freeze a cold-build command and complete reservation from
actual cold-build footprint data, including tools/copies/output/margin. The
obsolete 4 GiB estimate is forbidden. Use the default enforced resource
launcher and fresh namespace census below 95 GB/100 GB; build once unless a
real mismatch requires another construction. No compile is admitted here.

After a separate pre-flight, required evidence includes the named example
tests, fast/extended checks of the exact helper compilation inputs, real short
native begin/lifetime/existing-destination/changed-witness controls, loaded
executable identity and host monotonic/realtime observation controls. Exercise
unchanged canonical helper refusal and additional admission with real frozen
receipt files. Bind every command, raw bytes, failure, resource/stop/cleanup and
observer hash. Preserve original failures. Source controls and new helper CI
cannot be reused as shipped-product migration evidence.

Root alone may run the source-only registration cell after exact pre-flight,
default 1800 seconds and full 1 GiB reservation; **not run at registration**:

```sh
python3 -B tools/resource_group.py -- env PYTHONPATH=tools/qualification:tools \
  python3 -B -m unittest -v test_simulator_exposure
```

No native process or Cargo command is part of this cell. Full R3 still needs
fresh #41/#42/#45/#43/#46, complete runtime integration, admitted full-rate smoke,
real S1 setup/transition and other phase maxima with 25% margin, stopped
immutable checkpoint and separate grading. If measured values do not fit the
owner's 7,500-second deadline, request a concrete larger deadline with the
numbers; never shave the 7,200-second offered interval.
