# Registered finite native simulator witness controls

Prospective trust registration, 2026-10-13. This extends the
[source/lifetime protocol](simulator-exposure-witness-protocol.md) under
[C-081](https://github.com/kmosoti/FabricO11y/issues/59#issuecomment-6138310799).
[C-093](https://github.com/kmosoti/FabricO11y/issues/59#issuecomment-6139196487)
admitted earlier source cells only. This registration admits no source cell,
compile, native process, smoke, S1 or full acceptance execution. No result is
recorded here. Root freezes the complete command/receipts/resources, gets Pip's
preflight and checks #59 before every later cell and after every run.

## Invariant, fixture and scope

The opt-in native helper must publish a complete, exclusive pre-offer begin
file and keep successful native lifetime through both conservative end bounds.
Only `examples/spindle_sim.rs` may differ among Rust compilation inputs from
candidate `8fb6f413179b094288316e25845c137440ba23fa`. At this registration its
SHA-256 stays `2b2828fed73cfe654fadc6739e31c51b9004a42a5174f51be2cb08b161385264`.
Initial missing-begin red uses sealed native bytes without rebuilding them.
Preserve legacy summary, original events/transcript, finite offers, payloads,
ACK interpretation, TLS, retry behavior and option-absent behavior. In opt-in
mode pending/undelivered work must explicitly refuse successful completion.

[`simulator_native_controls.py`](../../../tools/qualification/simulator_native_controls.py)
is an explicitly invoked **loopback TLS test sink**, not a Fabric server. It
uses the existing certificate fixture, verifies bearer tokens, accepts actual
raw Batch protobuf, decodes with the unchanged independent query decoder,
checks two 512-byte logs plus the original short metric shape, retains exact
bytes and ACKs each stream in order. Configuration GET receives 304. Positive
cells use seed `0xA11FA001`, two seconds, two identities and two requested workers;
all four original sources, created events, responses and final ACK cursors are
checked against accepted raw-byte SHA-256. It keeps native logs unmodified.

The sink ACKs volatile diagnostic bytes. It makes no durable ACK, journal,
browser enrollment, control-plane, query, production network, rate precision,
7,200-second exposure or qualification claim. It does not append fake recovered
records or reinterpret the delivery oracle. The existing source/rate/delivery
oracles remain separate, unchanged prerequisites for candidate acceptance.

The controller holds the executable descriptor, hashes it, launches that held
ELF via `/proc/self/fd`, and independently observes `/proc/PID/exe` hash/device/
inode, PID/start ticks and boot ID while alive. It refuses preload/audit/library
override variables. Positive controls hold a live witness descriptor and bind
complete bounded JSON, mode 0400, regular single-link inode and unchanged final
bytes. Complete source/build/sysroot admission happens **before** this fixture;
its small authority tuple is an external expectation, not source proof.

## Separate source authority and intended cells

The [canonical expected tuple](simulator-native-witness-canonical.json) binds
DEB A's original source commit/tree/manifest/bundle, build receipt, sealed
simulator and original three-helper manifest. Its SHA-256 is
`23f3c7f259e5a4f3f3cfd36e59f20426f0ad62c9510fa2e2fe10a4f2e36445d1`.
Root checks actual canonical files/manifest and the unchanged cross-family
guard first. A rebuilt helper uses a different separately admitted JSON with
exactly the same seven field names and original canonical helper-manifest hash.
Root pins its actual bytes and the real build receipt. Complete compilation
path sets/bytes, commit/tree/bundle, environment/toolchain/dependency/sysroot
fingerprints and `validate_source_build` must agree with an independently
frozen expected record. None of the three canonical helper bytes is replaced.

Per-file registration budgets: native fixture at most 360 lines; new source
controls at most 100 lines; canonical JSON at most 16 lines; this protocol at
most 200 lines; existing protocol addition at most 6 lines. No production Rust,
Cargo/dependency/core, canonical guard, dump helper or existing oracle edits.
The eight new source methods check finite wire positives, every canonical pin,
separate authority, malformed authority, duplicate/truncated/unknown/invalid
wire, body and metric defects. They start no native process or server. The
existing 16-method suite retains literal 1 ns early-end, missing/unloaded/
changed identity, added/deleted compilation input and original-guard controls.
The literal 1 ns mutations are receipt arithmetic, not clock precision evidence.

Intended source-only cell, **not run**, expects 24 methods, zero skips/failures:

```sh
python3 -B tools/resource_group.py -- env PYTHONPATH=tools/qualification:tools \
  python3 -B -m unittest -v test_simulator_exposure test_simulator_native_controls
```

Root separately runs the normal documentation check/tests/hooks through the
same launcher. Source import, source tests and documentation results cannot
establish native lifetime. Every source cell reserves the full 1 GiB owned
allowance; native cells below separately reserve 1 GiB each and keep the default
1,800-second outer deadline. Root's namespace/free-space preflight remains
mandatory. Fixture subprocess waits are 10 seconds, or 75 for pending drain;
certificate creation has the existing 60-second per-call bound. The resource
launcher contains the entire descendant tree; no uncapped fallback is allowed.

After separate admission, exact initial native cell from the registered tree:

```sh
python3 -B tools/resource_group.py -- env PYTHONPATH=tools/qualification:tools \
  python3 -B tools/qualification/simulator_native_controls.py --case canonical-red \
  --binary /run/media/kmosoti/data/FabricO11y/results/installation-package-build/fresh-8fb6f413179b-01-deb-a/qualification-helpers/spindle_sim \
  --authority docs/experiments/benchmarks/simulator-native-witness-canonical.json \
  --authority-sha256 23f3c7f259e5a4f3f3cfd36e59f20426f0ad62c9510fa2e2fe10a4f2e36445d1 \
  --build-receipt /run/media/kmosoti/data/FabricO11y/results/installation-package-build/fresh-8fb6f413179b-01-deb-a/receipt.json \
  --out /run/media/kmosoti/data/FabricO11y/results/simulator-native-missing-begin-red-01
```

Expected red is exit 1 with receipt status
`expected-red: sealed-native-missing-begin-after-real-ACK`, **only after** native
exit 0, undelivered 0, loaded binding and four successful original ACKs. Exit 2,
setup failure, timeout, missing/malformed ledgers or incorrect source is
inconclusive, never that red. Preserve the actual red before any Rust edit.

After rebuilt-source admission, freeze eight explicit fixture commands with
the same launcher/options, separate helper ELF/build/authority hashes and
fresh per-cell output paths. Select `--case` as follows; controls expect fixture
exit 0 only when their registered condition is observed:

| Case | Required real observation |
| --- | --- |
| `positive` | Four ACKs, live/final bound witness, native elapsed >= 2 s and monotonic end >= upper begin bracket + 2 s. |
| `legacy` | No option/file; successful four ACKs and exact original five-key summary. |
| `duplicate` | Two `--begin-witness` arguments refuse before any request, with neither file published. |
| `existing` | Existing 0400 destination remains byte-identical; refuse before any request. |
| `workers` | Five identities/requested four partitions into three today; opt-in refuses before publication/offers rather than claiming four workers. |
| `pending` | Real TLS 503 retains both sequences per identity; native exit 1 and summary undelivered 4 after the original watchdog. |
| `early` | After actual live witness/offer, SIGTERM before boundary produces native nonzero; no successful trial. |
| `changed` | Deliberately alter held witness while native is alive; native nonzero. |

Four separate preflight refusal cells retain the positive command's frozen
expected hashes: alter authority bytes, point to an altered binary copy, point
to an altered build-receipt copy, or add `LD_PRELOAD=` inside its `env` command.
Each expects exit 2 before any helper launch; no library is injected by the
empty preload variable. Preserve stderr and the outer containment receipt.
Do not preload a clock shim, alter host time or use shortened 7,200-second
receipts. Such preflight cells test rejection only. Rejected-argument cells may
exit too quickly for `/proc` inspection; they prove refusal on the held pinned
ELF, while the independent loaded-process observation belongs to live cells.

## Atomic publication and order remain independent obligations

Polling sees complete bytes and final metadata; it does **not** prove atomic
no-replace publication or pre-offer order. The fixture explicitly records that
gap and does not fabricate `loaded.publication` flags for `validate_native`.
Before full admission, register and execute native file-operation controls
with raw observer/syscall evidence and explicit fault points: complete private
staging before hard-link publication, existing-destination refusal, final
0400/nlink1 after owned staging unlink, no final partial JSON while writing,
staging collision/symlink refusal, changed held descriptor, injected write/
sync/link/unlink failure, and publication completed before the first created
event. A transient two-link stage must not be graded as completed publication.

Intended Rust example tests, not present/run at registration, use prefix
`native_witness_` for these file/parse/order controls plus reversed/oversized
clock brackets, zero-duration and exact actual-partition refusal. Only the
sole helper Rust file may add them. Root must review a concrete syscall/order
observer and failure-injection patch before its separately admitted native
test command (`cargo test --offline --locked --example spindle_sim
native_witness_ -- --test-threads=1`). Read-only mode and equal hashes under an
owned-directory assumption are not adversarial immutability proof. A native
clock read and observed conservative lifetime establish neither exact anchor
precision nor arbitrary-run timing. Full true Fabric TLS/protocol/S1 remains
separate; this diagnosed test sink never replaces it.

## Cold helper build prerequisites and cleanup

Do not invoke the package builder: it rebuilds canonical packages. A separate
helper-only builder uses exclusive owned DATA source/target/registry/out and
Podman graph/run/tmp, a read-only Rust 1.99.0 toolchain, empty release target,
two Cargo jobs and `cargo build --offline --locked --release --workspace
--example spindle_sim`. Epoch is 1791841113; no inherited RUSTFLAGS. Rustc
commit is `b940084d7eb6a299eb4bfeb8e34901bc051e7ac4`, LLVM 23.1.1. Debian 12 base
digest is `7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587`.
Registry copy is 438,223,080 bytes, inventory SHA-256
`47646ea10bb1e5fe179fc11ce36e0fe11acb6f5dacd9aa7d09c0c9a6d18577ce`.

Before compilation, separately prepare/freeze the complete Debian installed
package versions, package archive hashes, installed files/symlinks/modes and
tool/loader/sysroot fingerprints; the historical complete sysroot hash is
missing. Bind an owned immutable prepared image and environment. Root/Pip
review its exact helper-only builder, source census and image fingerprints.
Use network-none/read-only build image with explicitly owned writable mounts,
verified 7/8 GiB high/max, no swap, inner 1,500/outer 1,800-second deadlines.
All preparation/download/build growth belongs to the full owned reservation.

Planning reserve is **9,663,676,416 bytes** (8 GiB construction plus 1 GiB
output/margin), not a proven cold peak. Historical A/B complete broader cold
constructions had final owned 1,948,722,650 bytes after transient sysroot
removal; maximum 30-second sampled DATA growth was 2,888,070,285/2,919,914,225
bytes, not transient upper bounds or target-only peaks. Interrupted host-dev
growth exceeded 10.1 GB; the obsolete 4 GiB estimate remains invalid. Complete
apt/sysroot hashes and an owned/full-DATA growth guard are prerequisites; if
a strict transient byte bound is required, quota or new measurement is still
missing. Stop below 95 GB; preserve the 100 GB ceiling and record sampled gaps.

Archive raw failures and actual source/build/observer/resource/stop receipts
before deleting only marker-owned staging, target/cache/sysroot and fixture
credentials. Keep minimal frozen ELF/provenance/evidence, verify all descendants
and TLS threads stopped and owned cleanup postconditions, then record DATA/free
space/RAM. Never touch frozen A/B, canonical manifests/packages, quarantine,
shared caches or another helper's branch. No build or cleanup is executed by
this registration. Native controls and source-only green do not authorize S1
or advance any candidate acceptance gate.
