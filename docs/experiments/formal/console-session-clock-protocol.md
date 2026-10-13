# Disposable controlled-time session checks

Status: prospective; **not run**. Issue #45's C2/ACCESS-1 requires packaged
idle and absolute session expiry. The existing native explicit-now test proves
its scoped adapter semantics; it does not establish packaged browser behavior.
This successor supplements the [console protocol](console-access-protocol.md)
without replacing its real reader TTL, signed rejection, recovery or other cells.
It is controlled-time expiry, not thirty-minute/eight-hour elapsed endurance.
It supplies no performance, outage, soak, delivery or package-build evidence.

The owner authorized this finite process-local fixture within existing deadlines.
Candidate artifact F is `8fb6f413179b094288316e25845c137440ba23fa`; actual fresh
#40/package payload identities, fresh #41, clean exact merged harness, independent
controls and reviewed admission are prerequisites for acceptance. Historical
ELF imports do not establish the new ELF's binding. No host clock mutation,
privilege, product flag or binary patch is allowed.

## Hook and control boundary

The sole interposed API is `clock_gettime(clockid_t, struct timespec *)` in a
fixture-only `.so`. Successful CLOCK_REALTIME reads in its original admitted
server PID add a validated nonnegative whole-second offset with overflow checks;
tv_nsec and underlying error behavior remain unchanged. All other clock IDs,
including MONOTONIC/RAW, are untouched. Do not hook time/gettimeofday/sleep/timer
APIs or silently broaden interception when the actual binary bypasses this path.
Resolve the underlying call using RTLD_NEXT with explicit initialization/error
handling. Invalid initialization or control permanently fails the fixture; an
ordinary-time fallback must never be credited as synthetic expiry.

The pinned [Rust SystemTime implementation](https://raw.githubusercontent.com/rust-lang/rust/b940084d7/library/std/src/sys/time/unix.rs)
selects CLOCK_REALTIME, and its
[Unix timespec path](https://raw.githubusercontent.com/rust-lang/rust/b940084d7/library/std/src/sys/pal/unix/time.rs)
calls libc clock_gettime. That source supports a hypothesis, not proof of the
new packaged ELF's import or running call path. Root must inspect that exact
ELF's interpreter/dynamic symbols before choosing interception. The
[loader's `--preload` option](https://man7.org/linux/man-pages/man8/ld.so.8.html)
applies to one executable rather than child execs, but direct
loader invocation keeps the loader as `/proc/PID/exe`, violating this protocol's
exact target-executable identity. The owner therefore selected the scoped
target-only LD_PRELOAD fallback before implementation. Set it only in the one
target exec's environment; the constructor clears LD_PRELOAD and the control
configuration before main, restores CLOEXEC and records actual fork/exec
isolation. Preserve the exact target ELF identity requirement. Never preload the
launcher, compiler, controller, grader, browser, driver, companion or normal
acceptance workloads. This selection is registered separately because it changes
the fixture's inheritance boundary; it is not runtime evidence that isolation
works.

Use a private anonymous AF_UNIX/SOCK_SEQPACKET socketpair, NONBLOCK+CLOEXEC,
with exactly one controller. Pass only the target endpoint to the server exec.
Before main/threading/fork, validate its type, consume mandatory initial offset0,
restore CLOEXEC, clear control/preload configuration from runtime environ and
record original PID/start identity. The packet is exactly24 bytes: eight bytes
`FCLKv1\0\0`, little-endian u64 sequence, little-endian u64 offset seconds.
Sequence starts0 and increases by1, offsets start0 and never decrease, maximum
offset28860, maximum64 packets. EAGAIN retains validated state; EOF, wrong type,
bad/closed FD, truncation/oversize, version/sequence/cap/decrease/overflow or
controller death permanently fails. Bound drains, retries, synchronization and
ledger work. A foreign fork PID returns unshifted time without reading control
or locking inherited synchronization; child exec must inherit neither helper
mapping, control configuration nor control FD.

## Independent evidence and literal oracle

`session_clock_oracle.py` imports no controller/shim transition logic. Its first
controls freeze the six offsets0,1,1799,1800,28799,28800 across clock IDs0/1/4.
Each probe row has exactly pid,sequence,offset_s,clock_id,raw_ns,returned_ns,
syscall_before_ns,syscall_after_ns. The raw call must lie in independently
captured matching raw-SYS_clock_gettime brackets, at most1s wide. Returned time
is raw+offset only for REALTIME and exactly raw otherwise, preserving nsec.
Missing rows, another PID, shifted monotonic, forged ordinary realtime, modified
nsec or mismatched sequence are named rejections. Wide/late clock observations
are inconclusive, not product failures or successes.

Compile a pinned Rust SystemTime/Instant probe and independent raw syscall
witness only after registration. A real500ms sleep has ordered raw monotonic
start-before/start-after/end-before/end-after brackets, total at most1s; Instant
elapsed must lie between the independent lower/upper elapsed bounds. Actual
compiler/toolchain, source/helper/probe/loader/libc hashes/build IDs and argv are
receipt inputs. A Rust probe alone does not establish the packaged server's
binding: the exact server PID must emit matching REALTIME calls during each
serialized request, bound to controller raw/monotonic request windows. Missing,
ambiguous, truncated or another-PID call evidence blocks acceptance.

Bind target executable and exactly one target helper mapping to independently
frozen hashes plus PID/start ticks/cgroup/dev/inode. Confirm target control type
and CLOEXEC. Independently observe controller/browser/driver/companion PIDs,
ordinary host-clock brackets and absence of helper maps/control keys/control FD.
At positive target offset1, a fork probe returns raw time without advancing
control sequence; after exec it has no helper, keys or FD. `/proc/PID/environ`
describes initial exec environment and cannot alone prove constructor unsetenv;
runtime/child observations must establish isolation. Forged maps, unpreloaded
target, helper/environment/FD escape and fork consumption are mandatory native
negative controls, not satisfied by fabricated pure-oracle records.

## Exact session boundaries

Use genuine resident/UV WebAuthn-issued memory sessions, never forged tokens or
edits to server state. Store only public session hashes and actual issuance,
expiry, baseline last-success and associated server-read timing. Pace every
dispatch through the unchanged550ms shared pacer;429 never substitutes for401
`sign in required`. Disable polling before the final session observations.

Use independent no-prior-cookie login contexts A/B so issuing A does not retire
B. A returns200 at its own baseline last-success+1799; untouched B returns exact
401 at its baseline+1800, with absolute expiry still later. A's successful read
refreshes A and cannot be reused as B's at-boundary oracle. Record different
actual baselines and choose request order accordingly; do not assume equal
issuance seconds.

In a separate disposable state, keep the same genuinely issued C token alive
at issuance+1000k for k1..28, then require200 at issuance+28799 and exact401 at
issuance+28800. Every preceding gap is below1800 and immutable public absolute
expiry equals issuance+28800, so idle expiry/reauthentication cannot explain the
final refusal. No token replacement, reissue/revoke or owner step-up is allowed.
The oracle requires all32 literal observations: A/B plus C's28 keepalives and
two boundary reads; actual token hashes, prior-seen progression, statuses,
refusal messages and server-call seconds must match independently bound inputs.
Wrong baseline/token, missing keepalive, changed boundary, expired200,
429/403/transport error and wrong error text are named checker defects.

Calculate forward-only offset from independent kernel observations and confirm
applied sequence. For adjacent target seconds keep offset unchanged and wait
in real time; never move backward to repair an overshoot. Late/ambiguous reads
remain inconclusive and require a new fixture. After actual server401, explicit
Check session must lock the real browser and clear protected memory while browser
Date remains ordinary. A client Date-only timer cannot prove server expiry.

## Containment, antirollback and completion

Preserve inner240s, existing expired-cursor1200s, outer1800s, browser4GB cap,
CPU200%,tasks512,no swap and512MiB fixture ceiling. Bound the clock ledger at
1MiB and fail on truncation. Clock manipulation applies only to disposable
expiry fixtures; all real reader, performance, outage and soak gates remain
ordinary-clock. Actual host rollback must be detected, not clamped. Forward
steps naturally age300s owner freshness/challenges; do ordinary setup first,
extend no policy and ensure TLS validity covers both ordinary and shifted time.
TLS/challenge/process errors never stand in for semantic expiry.

The future access watermark/telemetry and credentials make this state unsuitable
for reuse. Stop all descendants/listeners, archive raw public failures/hashes and
verify stop, then remove only owned fixture credentials, profiles, private keys,
future state, sockets and probe/helper build outputs. Never reset watermark or
recover/restart this state in ordinary time; preserve immutable package inputs.

Root first runs this exact prospective-control command on the registration:

```sh
python3 -B tools/resource_group.py -- python3 -B -m unittest discover -s tools/ui -p test_session_clock.py -v
```

The initial named availability assertion checks the declared Python controller,
C shim and Rust probe source paths. Its expected red is **missing fixture
mechanism**, not a product defect or behavioral rejection. No broken fallback
is invented. After that captured red, implement separately and rerun unchanged
literal controls. Availability-green is not interception or expiry evidence:
compiled actual-ELF/loaded-PID/raw-clock/frame/child-isolation controls, including
native unpreloaded rejection, remain mandatory before browser admission. All
these native and packaged controls are **not run** at registration. Freeze their
actual paths/hashes/argv in a subsequent preflight; missing tooling remains an
actionable prerequisite within scope rather than an invented passing result.
