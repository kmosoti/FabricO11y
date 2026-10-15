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

## Lifecycle and frozen-image regressions

Static review of mechanism `c8fbf41` found three defects before native admission.
The separate prospective controls retain the clock oracle and every existing
expected outcome. They run only inside the default resource group, using small
owned disk fixtures and restoring the test process's previous subreaper setting.

1. A leader exits after a same-PGID child installs SIGTERM ignore. Closing the
   fixture must force-stop and reap that owned group, even when leader wait has
   already completed. Test cleanup independently kills/reaps the child on red.
2. A literal fake compiler writes public stderr and a partial object, then forks
   a TERM-resistant child. A private clock trace leaves 0.75s of the unchanged
   240s deadline. TimeoutExpired must remain visible; descendants must stop and
   both captured output and partial build evidence must be archived before
   removing the owned build directory. No real compiler is run by this control.
3. Real owned executable/helper files change after their original literal hashes
   and device/inode witnesses are frozen. Mocked /proc metadata agrees with the
   changed paths; the checker must still refuse against `expected` frozen fields:
   executable_sha256/executable_identity and helper_sha256/helper_identity.
   The exact refusals are `isolation-target-frozen` and `isolation-helper-frozen`.
   Where the legacy checker lacks that witness parameter, the test invokes its
   unchanged path and records actual acceptance as the named assertion failure.

These fixtures are harness-defect controls, not packaged expiry or compiler
qualification. Root records their actual red before a separate production fix.

An additional identity control replaces each frozen executable/helper path with
an identical-byte file on a new inode. It separately forges the loaded helper
mapping's device and inode while retaining the original file. Original hashes
alone cannot admit either replacement: executable replacement must refuse with
`isolation-target-frozen`; helper replacement or mismatched mapped device/inode
must refuse with `isolation-helper-frozen`. Existing literal controls and the
clock oracle stay fixed. The fake compiler and accelerated test-only monotonic
trace above are neither a real 240-second run nor real compiler-binding acceptance.

## Two serial state fixtures and browser cleanup registration

Prospective scope acknowledged by [C-069](https://github.com/kmosoti/FabricO11y/issues/59#issuecomment-6137789072)
before implementation. Controls and packaged browser acceptance are **not run**
on this registration. The older single-PID `check_sessions` controls remain
byte-identical compatibility checks. They cannot admit the packaged adapter:
the original literal order followed by C's absolute boundary exceeds the
offset ceiling in one process. Use two freshly created serial states while
retaining all 32 observations, statuses, error strings and wall-second bounds.

The [actual registration result](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-browser-session-fixture-registration-red-results-01)
at `b1e7e83` records four missing-checker assertions and five actual-finalizer
counterexamples; independent cleanup completed. The AB/C oracle is now
implemented separately using the unchanged session-semantic validator and
observed rows. Its [source checks](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-browser-session-source-results-01)
are recorded separately. Finalizer ownership needs the prospective amendment
below; native timing and browser acceptance remain pending.

The new independent interface is
`check_session_fixtures(rows, sessions, fixtures) -> None`; rejection raises a
named `ValueError`. It must not mutate any input or rewrite observed PIDs to
feed the legacy single-PID checker. Fixtures have exactly the keys `AB` and
`C`; the fixed session mapping is A/B to AB and C to C. A/B share AB's positive
integer server PID; C's positive integer PID differs. Boolean and floating
values are not integer identities or timestamps. There is no implicit mapping,
single-PID fallback, extra fixture or unbound row.

Each fixture record carries the following independently collected inputs:

| Fields | Required binding |
| --- | --- |
| `candidate_commit`, `package_sha256`, `executable_sha256`, `helper_sha256` | Full source commit and lower-case SHA-256 identities, identical across AB/C, separately admitted against immutable package/source inputs |
| `process_binding` | Actual `session_clock.process_binding` receipt: PID, positive start ticks, nonempty cgroup, matching executable/helper hashes, libc path/hash and nonempty helper mappings; original dev/inode/hash/maps checks remain mandatory native admission |
| `state_before` | Canonical absolute state path, device/inode, `created_monotonic_ns`, `observed_monotonic_ns`, `initial_entries=[]`, `copy_source=None`, `descriptor_held=True` |
| `state_after` | Same canonical path/device/inode and a later `observed_monotonic_ns` inside that fixture's live interval |
| `run_started_monotonic_ns`, `deadline_monotonic_ns` | Identical across both fixtures; deadline is exactly run start plus 240,000,000,000 ns |
| `started_monotonic_ns`, `stopped_monotonic_ns` | Actual start and completed stop observations inside the one global deadline |
| `owned_groups` | Nonempty, duplicate-free inventory of launched private `{pgid, start_ticks}` identities, including the bound server |
| `stop_observations` | Exactly one matching identity per launched group with actual `observed_monotonic_ns`, integer `population=0` and boolean `confirmed=True`, after its requests and no later than completed stop |

AB and C state paths must differ, and their `(device, inode)` pairs must differ.
The actual adapter exclusively creates each directory without reusing a name,
holds its protected descriptor, observes empty contents before server start and
keeps the original identity through the later observation. It performs no state
copying. Creation precedes the empty observation, which precedes server start.
Metadata assertions alone do not prove that no copying occurred; actual adapter
ordering and protected descriptor observations are prerequisites. Literal
checker receipts do not constitute filesystem or native process evidence.

All AB owned groups must be confirmed empty before C starts. AB completed stop
strictly precedes C start. A prior stop boolean, leader exit, omitted group,
duplicate observation, wrong start identity or still-populated group cannot
satisfy this ordering. The final outer resource-group stop receipt is additional
evidence and cannot replace either fixture's stop observation.

Every row retains the original fields and adds actual unshifted
`request_before_monotonic_ns`, `server_call_monotonic_ns` and
`request_after_monotonic_ns`. The matching fixture contains the entire request
window; the server call lies inside it. Dispatches preserve the unchanged 550ms
minimum gap over the whole sequence, including the AB/C transition. Do not
reset the pacer or global deadline for C. Setup, both serial fixtures and their
stops must fit the same inner 240 seconds. Any overrun is a preserved failure,
not permission to extend time. Outer 1800 seconds, browser 4GB, CPU200%,
tasks512, no swap, 512MiB fixtures, 1MiB ledger, 64 frames and offset at most
28860 remain unchanged. Each controller starts a fresh forward-only sequence
at zero and independently passes the existing frame/native isolation checks.

**Native timing prerequisite:** `server_call_monotonic_ns` must be a direct
unshifted clock witness from the same REALTIME shim invocation, bound to its
PID/TID and applied sequence. The current native ledger lacks that witness.
Controller receive time and wall-time subtraction cannot reconstruct it.
Register and independently control the actual adapter before browser admission,
preserving the original clock-return/prototype semantics, at most 256 bytes per
record and 1MiB total. If an exact witness cannot fit, a directly witnessed
monotonic bracket needs a separately registered shape before implementation.
The synthetic exact timestamps below establish neither interception nor native
time attribution. Genuine resident/UV-issued sessions, trusted production URL,
request-to-shim joins, secret-safe archives and all prior native controls remain
separate admission prerequisites; #41 must complete before browser acceptance.

`SessionFixtureBindingTests` supplies one valid independent literal receipt
pair, verifies no input mutation and rejects exact-key/image/process mismatch,
reused PID, crossed/unbound row, aliased/copied/nonempty/replaced state, missing
or duplicate stop evidence, nonzero population, overlap, deadline reset/overrun,
out-of-window reads and a 549,999,999ns gap within a state and across the state
boundary. Original missing keepalive, idle/absolute seconds, last-seen progress,
401 `sign in required` and 429/expired-200 rejections remain fixed.
Fixture diagnostics are `session-fixture-keys`, `-images`, `-pid`, `-process`,
`-state`, `-fresh`, `-deadline`, `-order`, `-stop`, `-row`, `-window` and `-pacing`.
Each suffix uses the full `session-fixture-` prefix. Original semantic
diagnostics remain unchanged. The initial named callable assertion is expected
to report **missing explicit two-state session fixture checker**. Until that
mechanism exists, an assertion at this point is availability evidence, not a
demonstrated malformed-receipt rejection.

`BrowserFixtureCleanupTests` executes only the actual `live_browser_check.main`
finally AST with synthetic owned processes. No browser, package, server or main
setup is launched. A parent records its fork child's PID/start identity
atomically before releasing a gate for risky child setup. The fixtures have
private process groups and finite 45-second lifespans; a test-only Linux
subreaper and pidfds permit independent cleanup without signaling reused PIDs.
The prior subreaper setting is restored. Scratch is removed only after all
owned process identities/groups and the sampler are confirmed stopped; missing
identity or unconfirmed cleanup retains the control directory. A setup or test
body exception also retains it after independent pidfd stop/reap, even when that
cleanup succeeds. Before any successful-directory removal, the control writes
an exclusive bounded 32KiB `cleanup-evidence.json`: actual identity/stop/reap
outcomes, bracketed group-zero observations, sampler/subreaper outcomes, bounded marker/script/log
identities and SHA-256 hashes, and the original exception type/message. Failed
directories retain those raw files and the generated literal child script;
stderr identifies the directory for root archival. The finalizer under test may
already have erased its own `work` subtree; record that fact without inventing
replacement evidence. Successful controls alone remove confirmed-clean scratch.

The literal controls require cleanup after leader exit, force-stop of a
TERM-resistant child after leader termination, timeout escalation, and attempts
for every owned group after one real group's stop is deliberately refused.
A stop refusal retains scratch and cannot claim stopped groups. A real bounded
sampler that remains alive after the unchanged two-second join likewise retains
scratch and cannot claim cleanup. An empty-process positive control must finish
the actual finalizer, stop its sampler and publish both public JSON receipts.
The timeout control explicitly shortens only its real `Popen.wait` calls to at
most 0.2 seconds; production 15-second TERM and five-second KILL waits remain
fixed. This is a descendant-cleanup control, not elapsed native timeout evidence.
The five negative cleanup methods may demonstrate existing cleanup defects
only when root actually runs them and confirms setup reached the finalizer.

Root runs the ten new methods through the launcher after publishing the exact
source pins, capped driver and a fresh full reservation/census:

```sh
python3 -B tools/resource_group.py -- env PYTHONPATH=tools/ui python3 -B -m unittest -v test_session_clock.SessionFixtureBindingTests test_role_matrix.BrowserFixtureCleanupTests
```

After preserving the actual red, implement the admitted correction separately
and rerun these unchanged controls plus both complete existing test files.
This source-check command does not authorize real browser acceptance. Actual
package/helper/process/maps, state/timing and owned-group receipt bindings need
their own frozen native preflight; passing fabricated metadata never supplies
that evidence.

## Delegated browser launch ownership amendment

Prospective scope is [C-084](https://github.com/kmosoti/FabricO11y/issues/59#issuecomment-6138580814),
acknowledged in [X-140](https://github.com/kmosoti/FabricO11y/issues/59#issuecomment-6138607824).
This supersedes the proposed PIDFD process-group signaling mechanism. It changes
cleanup authority to exclusively created delegated cgroup subtrees, retaining
the five original failed-stop outcomes, empty positive, exact child reaping and
all prior session/clock/status controls. Registration is **not run**. Production
finalizer, native clock helper and packaged adapter remain unchanged here.

Reuse `tools/bench/labs/completion/cgroups.py` and `enter_group.py`, with the
outer launcher's `--delegate`. Before delegation, hold the fresh owned service
directory descriptor and verify its aggregate 4GB/3.5GB high, CPU200%, tasks512
and no-swap limits. `delegate()` moves the controller into `supervisor`; that
directory is not the owned service ancestor and its direct limits do not
replace the retained parent aggregate cap. Hold the parent identity throughout
every launch and cleanup. Exclusively create each subgroup beneath that parent,
set/read its explicit limits no larger than the parent, and hold its directory
descriptor before launch. `enter_group.py` verifies placement before executing
the target; descendants inherit it across `setsid`. Refuse missing delegation,
subgroup, kill capability or actual placement. No numeric PID/PGID signal
fallback is admitted.

Two narrow functions constitute the prospective source interface:

| Interface | Owned inputs and required behavior |
| --- | --- |
| `bind_browser_group(parent, group, proc)` | Validate held parent/subgroup identities, exclusive creation provenance, exact `Popen` object, original PID/start ticks, held matching pidfd and actual placement; return defensive value copies as `{'parent': parent_record, 'group': group_record}` without signaling |
| `stop_browser_groups(parent, bindings)` | Revalidate the retained registry and directory identities before signaling; attempt every owned group despite another failure; retain separate kernel-empty and child-reaped observations; aggregate failure as `RuntimeError('browser-group-stop')` |

The parent record is `{path, fd, dev, ino, deadline_monotonic_ns}`. The group
record is `{path, fd, dev, ino, exclusive, process, pid, start_ticks, pidfd}`.
Paths are canonical owned directories; `fd` is the held no-follow directory
descriptor and `dev/ino` are its original identity. The group records the exact
launch `process`, positive original `pid/start_ticks`, and separately held
matching `pidfd`. The launcher owns these descriptors through stop/archive;
the independent test guardian holds different pidfds. The returned records
must not alias mutable input dictionaries. The returned group additionally
retains `members`, independently witnessed `{pid, start_ticks, pidfd}` records
from the live admitted subtree; the source fixture requires its actual child
to be bound before it becomes a zombie. The `exclusive=True` observation
records an actual successful no-reuse `mkdir` in the owned launch path. Metadata
alone does not prove creation provenance or pre-exec placement: native source
ordering and actual kernel observations remain prerequisites. These functions
do not accept arbitrary user-selected cgroups as cleanup authority.

Binding refusals are exact `RuntimeError` diagnostics: `browser-group-delegation`,
`-identity`, `-capability`, `-placement`, `-ownership`, `-parent` and `-deadline`,
each with the full `browser-group-` prefix. A refusal occurs before any signal
or kill-file write. Replaced parent/subgroup paths, mismatching held descriptors,
forged `Popen`/PID/start/pidfd and stale registry records cannot authorize stop.
An unbound group stays untouched while the other registered groups still receive
their independent stop attempts.

Use only PIDFD-bound TERM for currently witnessed members of the owned subtree,
with the unchanged 15-second grace. Then the retained subgroup's `cgroup.kill`
receives `1` if live members remain, followed by at most five seconds to observe
`cgroup.events populated 0`. The [kernel contract](https://docs.kernel.org/admin-guide/cgroup-v2.html)
includes descendant subgroups and handles concurrent forks/migrations. It also
excludes zombies from `cgroup.procs`; `populated 0` alone is not child reaping.
Keep the owned test/runtime subreaper setting through all launches and restore
its previous value after cleanup. Reap exact adopted identities (for example,
[PIDFD-bound `waitid`](https://man7.org/linux/man-pages/man2/waitpid.2.html)),
and separately confirm their `/proc` identities are absent. Record both facts
before removing any private scratch. Stop the sampler with the unchanged
two-second join; unconfirmed sampler/group/reaping retains scratch and cannot
publish a successful cleanup claim. Archive bounded diagnostics before a close
failure. All stop attempts continue after another group's refusal.

A binder-time snapshot does not establish ownership of later runtime forks or
guarantee their zombie reaping. The source fixture has one independently known
child already live at binding; it cannot establish native completeness. Before
production/native admission, register a truthful retained descendant collection
and exact reaping strategy that covers later forks and `setsid`, without
arbitrary foreign PIDs or broad `waitpid(-1)`. A modeled zero population never
substitutes for that missing ownership evidence.

The single `deadline_monotonic_ns` is run start plus 240,000,000,000ns. Both
serial states, delegation/setup, dispatch and stop share it; no reset at C.
Local 15/5/2 bounds never extend that deadline. Any overrun is a preserved
failure; bounded identity-safe failure cleanup still occurs under outer1800.
Original 550ms global pacing and every frame/offset/ledger bound remain fixed.

The existing `BrowserFixtureCleanupTests` retain all six methods/outcomes.
Their failed-stop injection now covers the old `killpg`, owned PIDFD signals
and the modeled subgroup kill write, with the same requirement that one real
group remains living while all other groups are attempted. The old timeout
continues to shorten only real `Popen.wait` to 0.2s and require the requested
production15s path. `DelegatedBrowserOwnershipTests` adds seven methods:

- one explicitly named binder/stopper availability assertion;
- seven literal delegation/group/kill/placement/creation/parent/deadline defects;
- seven original process/descriptor/directory identity defects;
- an actual `setsid` TERM-resistant child after its leader exits;
- an actual adopted zombie when modeled population is zero;
- bounded stuck population with scratch retention and exact child reaping;
- four stale-registry defects, including replaced parent and subgroup paths.

Until both functions and existing cgroup plumbing exist, the two defect tables,
stuck-population and stale-registry methods are explicit dependent skips. They
do not report rejected input when their implementation is absent. The two new
real-process methods still execute the actual old finally. Prospective unchanged
production counts are 13 methods: one empty positive, five old and two new
cleanup failures, one named availability failure, and four dependent skips.
These are predictions, not recorded outcomes. After a separate admitted repair,
all13 must execute with no skips/failures.

The source fixture exclusively creates ordinary scratch directories and held
directory FDs, models cgroup controller/cap/event/procs/placement reads, and
intercepts only exact owned kill-file writes (path or FD) into signals through
its own retained pidfds. Its process/zombie/reaping observations are real;
its cgroup placement/population/subtree-kill observations are source mocks.
This cannot demonstrate kernel delegation, real pre-exec placement, concurrent
subtree kill or actual aggregate enforcement. A later admitted delegated
kernel cell must independently establish those facts with the exact source,
parent limits, owned subgroup identities and raw stop/reap observations.
Original retained failure directories are not edited or removed. New failures
retain generated scripts/atomic markers/logs and the bounded independent cleanup
evidence after all owned processes are stopped/reaped; successful fixtures alone
remove confirmed-clean scratch.

Root's prospective source-only command (fresh census/full1GiB reservation,
default16/20GiB/no-swap/outer1800, exact frozen driver/source preflight):

```sh
python3 -B tools/resource_group.py -- env PYTHONPATH=tools/ui python3 -B -m unittest -v test_role_matrix.BrowserFixtureCleanupTests test_role_matrix.DelegatedBrowserOwnershipTests
```

This cell does not use `--delegate`: all its cgroup interfaces are source mocks,
while its real synthetic descendants remain under the enforced outer service.
The future actual kernel/browser cell requires `--delegate` and separate scope.

## Native cleanup integration follow-up, 2026-10-15

This prospective follow-up retains all thirteen source controls and their exact
outcomes. It registers a dedicated whole-fixture wait domain in the existing
controller: before launching persistent children, require the kernel's own-child
`ECHILD`, ordinary SIGCHLD disposition without `SA_NOCLDWAIT`, a saved/verified
subreaper setting, stable reader PID/mount/cgroup namespaces, and held parent
aggregate limits. The sampler reads files only; only the controller waits.

Each persistent launch exclusively creates a bounded subgroup, holds its
no-follow directory and leader pidfd, executes the existing placement helper,
and binds after observed placement. No PID/PGID signaling fallback is admitted.
The retained subgroup's `cgroup.kill` handles later live forks; after all groups
are kernel-empty, non-consuming `waitid(P_ALL, WEXITED|WNOHANG|WNOWAIT|__WALL)`
discovers adopted exits. Every returned child must independently join a held
owned subgroup by its actual `/proc/PID/cgroup`, controller parentage, start ticks
and pidfd before exact `P_PIDFD` consumption. A foreign child is neither consumed
nor signaled. No-event means pending, never empty; only `ECHILD` closes the domain.
This removes the need for a live fork snapshot/freezer as a completeness claim.
The kernel-empty and own-child-empty observations remain separate evidence.

Bound the final adoption drain to the existing five-second stop phase and the
original global deadline; timeout retains scratch. Restore subreaper state only
after confirmed cleanup. Native success requires actual absence of each known
original PID/start identity and retained raw namespace/group/wait observations.
No concurrent waiter, debugger or foreign child launcher is admitted. Support is
limited to the inspected Linux x86_64/glibc sigaction layout; unsupported ABI or
signal ownership refuses before launches. Original session timing/pacing,
private-file bounds and all independent oracles stay unchanged.

Four prospective native cleanup controls use real delegated groups: ordinary
TERM-resistant descendant; late double-fork/setsid after binding; late child
already zombie and absent from cgroup.procs; and a same-UID foreign direct child
that must remain unconsumed after refusal. The fixture independently holds its
child pidfds and records actual /proc identities, group events and exact exits;
its bounded guardian reaps only those identities on failure. Existing thirteen
modeled cgroup controls still run separately and are not native evidence.
Native control invocation is registered as `python3 -B tools/resource_group.py
--delegate -- env PYTHONPATH=tools/ui python3 -B tools/ui/browser_cleanup_native.py
--out DATA/results/browser-cleanup-native-01`; source/final command hashes and
full1GiB reservation are frozen before execution. Combined limits are unchanged:
3.5/4 GB high/max, CPU200%, tasks512, no swap, 240 seconds inside outer1800.

The earlier unexecuted +190-line design estimate was insufficient to express
complete retained descriptor ownership, native wait-domain admission, exact late
child reaping and bounded failure receipts readably. This follow-up permits at
most +360 net lines in live_browser_check.py from 90cdf68 and a native fixture
of at most 240 lines. No existing outcome, deadline or oracle is relaxed.

## Native request-window binding amendment

Prospective registration; no new native or browser result is implied. The frozen
six-u64 `fabric_clock_last` ABI and exact probe oracle rows remain unchanged.
Each native ledger row additionally records positive Linux TID, a process-wide
contiguous successful invocation index beginning at1, and unshifted native
CLOCK_MONOTONIC nanoseconds immediately before and after the actual underlying
clock call. These brackets surround the wall call itself, not controller receipt
time. A separately named timing accessor binds probe records to independent raw
kernel MONOTONIC brackets without altering the old ABI. Preserve the actual
underlying return/errno, other clock IDs, 256-byte record, 1MiB total ledger,
64 frames and offset28860 limits. Overflow is a refusal, never truncation.

For a serialized HTTPS request, capture controller raw-kernel MONOTONIC before
send and after the bounded response read. Because every native ledger send
completes before the intercepted call returns, draining the channel under the
receiver's mutex after response completion captures every call that could have
contributed to that response. Preserve the complete ledger and contiguous indices;
select every REALTIME invocation whose native interval overlaps the request.
Require a nonempty set, all intervals entirely within the request bracket, all
from the admitted PID/frame/offset, and every returned wall second equal to the
registered target second. Select the first actual invocation only as the scalar
witness required by the existing32-row oracle; retain the entire candidate set
in each row. This supplies complete possible-call-set evidence, not a claimed
unique middleware/authorization call. Missing calls, gaps, wrong PID/TID/frame,
straddling intervals, mixed seconds, malformed/oversize records and absent
interception must refuse. Controller receipt timestamps never substitute for
native monotonic observations.

Literal source controls register: a valid multi-call window; missing REALTIME;
call-index gap/duplicate; zero TID; wrong PID/frame/offset; shifted wall or
monotonic bracket; overlapping straddler; mixed second; and record/ledger overflow.
Native admission must retain every old probe equality and independently bind the
new timing accessor to raw-kernel monotonic brackets. Compile/probe controls run
before an actual-server request-join cell and before any browser expiry cell.

The server adapter starts the ordinary Python cgroup-entry helper without
LD_PRELOAD/control environment. Only after confirmed placement does that helper
set the two values immediately before same-PID exec of the exact admitted ELF,
passing only the protected channel FD. The existing constructor consumes offset0,
clears keys and restores CLOEXEC in the server. Source controls must reject
preloading the entry helper, absent control FD, and an unbound server executable.
No Python launcher is preloaded. Actual executable/helper mapping, control FD,
companion isolation and native request timing remain required run evidence.
