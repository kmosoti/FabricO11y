# Dedicated browser ownership within an R3 campaign

Prospective registration; no source/native result or acceptance is claimed.
This extends [the controlled session protocol](console-session-clock-protocol.md)
and retains its tested browser ownership functions and ECHILD admission.

The R3 controller has other live campaign children. It cannot itself use the
browser controller's initially empty wait domain. A dedicated Python supervisor
starts before its browser child, establishes the existing isolated subreaper
domain, and invokes the existing `browser_group_launch`,
`stop_browser_groups`, `browser_reap_domain` and `end_browser_domain` functions.
It never accepts arbitrary PID/PGID signal authority.

The parent derives its actual original `fabric-work-*.service` ancestor from
kernel cgroup placement and retains that ancestor's no-follow directory FD and
identity. Its actual aggregate memory limit must be at most 6 GiB with no swap;
high, CPU and task limits remain those of the registered R3 envelope. It creates
one new descendant exclusively and retains its descriptor. This descendant is
bounded to 4,000,000,000 maximum/3,500,000,000 high bytes (kernel page rounding),
CPU 200%, 512 tasks and no swap. Before controller delegation its only process
must be the fresh supervisor. Its `fabric-work-browser-controller-<random>` name
preserves existing helper syntax; authority comes from actual ancestor identity,
exclusive creation, retained descriptors and verified placement, not that name.
The existing `enter_group.py` exec wrapper performs placement; no threaded
parent uses `preexec_fn`. Nested delegation must use the same inherited
controllers and leave the original aggregate cap intact.

R3 additionally supplies its already registered, empty 1 GiB/850 MiB browser
budget with its retained no-follow descriptor, identity and actual exclusive
creation observation. CPU 200%, 512 tasks and no swap remain enforced there.
The new 4 GB controller domain is nested beneath that budget and inherits its
stronger 1 GiB effective aggregate bound; the inner 4 GB value is never described
as the effective limit. Both ancestors, actual limits and placement are retained.
Changing or substituting the supplied budget must refuse before launch. No
browser process may move to the old sibling budget outside its owned domain.

One private inherited SOCK_SEQPACKET pair carries bounded JSON ready/stop/closed
messages, each at most 8 KiB. Ready means the supervisor has actually acquired
and bound the driver subgroup/identity. It does not mean WebDriver is ready;
the bridge still probes its actual `/status`. Startup, EOF, malformed messages,
driver exit and the run's original absolute monotonic deadline enter the same
bounded cleanup. The supervisor's ready and cleanup receipts contain no tokens,
cookies or response bodies. The parent bounds startup at 10 seconds and normal
shutdown at 20 seconds and the caller's shutdown-phase deadline, whichever is
earlier, without extending the R3 deadline. The stop packet may only tighten the
original absolute deadline; both retained parent records use that tighter bound.
After expiry, owned-domain kill/reap attempts are failed cleanup, never credited
as completion of the expired phase.

Only a clean supervisor exit plus its separate populated-zero, exact child
absence and own-child ECHILD observations can establish browser cleanup.
The parent independently rechecks the retained domain identity and population.
If the supervisor dies or cannot publish complete cleanup, the parent kills
only its held owned descendant through cgroup.kill, reaps its exact supervisor,
retains scratch and fails the campaign. This fallback is not a browser-only
cleanup success. R3 must stop all other campaign children and close its root
owned wait domain before checkpoint/removal; populated zero alone cannot prove
that adopted zombies were reaped.

Register separate source and delegated native controls before execution. Native
controls cover normal driver/descendant cleanup in both the direct and preserved
1 GiB budget shapes, startup refusal, supervisor death before ready and death
during cleanup in the nested shape, plus wrong/changed ancestor refusals with
no launch or descriptor leak: seven controls total. An independent fixture guardian
owns the complete test wait domain and verifies exact descendant absence and
ECHILD even when the supervisor cannot complete. Retain failures and resource
receipts. Existing thirteen source controls and four native cleanup cases must
retain their exact outcomes at the resulting source revision; their historical
receipts remain bound to their original revisions.

Harness-only deterministic `before-ready` and `during-cleanup` fault points
publish a bounded PID/start marker and SIGSTOP the supervisor itself. The
independent fixture guardian authenticates that identity with its own pidfd and
kills it. The second point follows subgroup stop and precedes late-child
adoption draining, exercising the distinction between kernel population and
reaping. Ordinary bridge callers select no fault point.

This registration does not change product authentication, package bytes,
session expiry rules, polling continuity, R3 deadlines or acceptance gates.

## Native browser child entry handshake

Prospective harness protocol amendment, 2026-10-18. The retained engineering
probe 04 stopped before browser launch: native helper work completed, then
`browser_group_launch` failed while binding the child identity and the existing
cleanup reported an unregistered child. Preserve that failure and its raw
diagnostic; it is not browser or acceptance evidence.

The entry path will use a bounded `xtask browser-enter-exec` command from an
explicitly caller-pinned immutable helper binary. The helper receives the exact
owned cgroup-v2 directory, one private READY write descriptor, one private
RELEASE read descriptor and an absolute target argv. It must admit a canonical
cgroup-v2 directory and open `cgroup.procs` without following symlinks, move
itself into that group and verify its own `/proc` cgroup placement before
writing the single READY byte. READY framing is exactly `R` followed by EOF:
the helper closes its READY writer immediately after `R`, and the parent waits
for that EOF under the same monotonic deadline. The helper then waits for the
single RELEASE byte using a monotonic deadline no longer than five seconds.
RELEASE framing is exactly `G` followed by EOF: the parent closes its writer
immediately after `G`, and the helper waits for that EOF before exec. EOF before
the byte, malformed or extra framing, a writer held open through the deadline,
wrong placement and deadline expiry are failures and never exec the target. The
helper requires distinct inherited control FDs greater than 2 with the correct
directions and closes both before target exec. It preserves target argv,
environment, standard streams and unrelated explicitly inherited descriptors.
The existing session-clock wrapper remains the target when controlled time is
selected, so its current environment and clock descriptor behavior is unchanged.

The parent creates the subgroup exclusively and the private pipes, launches
only the exact pinned helper, and immediately retains the Popen object, pidfd,
start ticks and owned subgroup descriptor. After READY it verifies the same
process identity, `/proc` placement and existing `cgroup.procs` membership using
the unchanged identity and membership checks. It records the valid binding
before writing RELEASE. Subgroup names come from a per-domain monotonic launch
attempt counter, incremented before each subgroup creation; a failed attempt
consumes its name, so the next launch cannot collide with an earlier retained
or cleaned subgroup. The domain launches serially and does not use a generic
allocator or retry failed names. Any failure before release retains the provisional
spawn record and may clean only that exact Popen/pidfd and newly owned subgroup;
it never signals a raw PID or PGID, adopts/reaps an unknown child, weakens the
foreign-child guard or releases an unbound child. There is no sleep or retry
substitute for the handshake. Existing process, namespace, cgroup and deadline
guards remain mandatory.

The helper path and SHA-256 are explicit browser inputs, copied and rechecked
through the existing immutable-binary snapshot path before any group launch.
The browser receipt records the caller path, pin, snapshot path and snapshot
digest. It may be the same xtask binary as the console-source helper only when
the independently supplied path and digest match exactly. No PATH lookup,
runtime compilation, helper self-reported pin, product-package change or CI
policy change is part of this amendment.

Before the correction is admitted, one finite Linux integration fixture must
invoke the actual parent launch seam and native helper under the existing owned
cgroup resource boundary. Its positive uses immediate `/bin/true`: READY must
precede parent validation; exact identity and target membership must be present
before RELEASE; a test-only `os.write(G)` guard checks the binding list, live
leader identity and exact target `cgroup.procs` membership at the release call.
An injected append-after-release source variant must be refused by this guard
before G is written and leave no child behind. Then exit, exact leader reap,
empty subgroup and clean outer stop are required. Named controls cover wrong
placement into a separately owned sibling subgroup (no RELEASE and cleanup of
only retained identities), READY followed by release-pipe EOF, bounded RELEASE
timeout, and the existing foreign-child rejection. A cleanup fallback control
injects exactly one `TimeoutExpired` from the actual Popen wait after pidfd
signaling, then requires the real wait and owned-cgroup cleanup to succeed using
the same retained pidfd. Each control records whether target exec occurred,
exact child identity and cleanup state. The parent fixture must not require a
browser runtime in Rust unit tests; focused unit controls cover framing,
descriptor direction/distinctness and timeout parsing, while this one Linux
fixture establishes actual cgroup placement and parent/helper composition.
Record each cell's actual elapsed time, peak RSS and cleanup state as bounded
composition diagnostics, without ranking against an unbarriered launch.
Preserve all failures and raw logs. These controls establish the entry protocol
only; they give no browser, package, product or release acceptance credit.

## Hosted CI helper binding adoption

The `fast-checks` hosted job must exercise the ignored Linux entry fixture after
building workspace binaries, then compute the caller's SHA-256 for the current
`$FABRIC_STORAGE_ROOT/cargo/debug/xtask` artifact. Both existing Chrome
`live_browser_check.py` invocations—the positive passkey/HTTPS check and the
exact API-cache negative—receive that explicit helper path and digest. The
fixture runs through the delegated resource launcher; it retains its release
ordering, placement, cleanup, and metric output. CI contract tests reject a
missing fixture launch or either missing helper pair while preserving the
existing positive and exact-negative commands and required job names. This is a
hosted harness-verification policy change only: it adds no acceptance result,
changes no package, browser oracle, negative cause or required registry check,
and does not qualify a product or release.

## Production health-wait scope correction

Prospective harness regression, 2026-10-18. The retained Chrome #45 baseline
`results/replacement-tls-01-browser-chrome-baseline/browser.json` has SHA-256
`ab6439c87bb0cb35d8c4792d7e768d656c5642254c262604e26cf863441820c1`, exit 1,
154 recorded checks, and `NameError: name 'healthy' is not defined` after the
offline owner-recovery command. Its outer resource record is
`evidence/fabric-work-e82276d87ea2492e8ad596f8c6a0d9f0/.resource-command.json`
and reports exit 1. Preserve this failed baseline as partial evidence, not
acceptance. The production source at the failure remains unchanged.

The smallest correction moves the existing production TLS health predicate to
`main` scope and gives it an explicit process argument. Keep its dead-process
refusal, CA-backed opener, one-second request timeout, HTTP-200 predicate and
`OSError`/`URLError` refusal behavior. The initial server wait and the offline
recovery and shell-lifecycle restart waits must each close over the current
`server` process in `wait(lambda: healthy(server), label)` while retaining their
existing labels and 25-second default wait deadline. Do not capture the initial
process for a later restart.

Before changing the source, a Rust integration test must parse the actual Python
AST, extract the actual main-scoped helper and the three actual wait expressions,
and invoke those expressions using a stable fake TLS opener, fake process and
immediate fake wait. It uses no network, browser, server or fixture. Against the
current source, evaluating the actual restart wait must produce the observed
`NameError`; after the correction, require HTTP 200 true, non-200 and
`URLError` false, a dead process raising before any request, and both restart
waits probing the replacement process. This regression does not change product
behavior, acceptance criteria or independent browser/security oracles. A passing
source control does not repair or relabel the retained Chrome #45 result.

## Shared production fixture consumer mapping

Prospective integration correction, 2026-10-15: CandidateFixture must select the
supervised browser lifecycle for ordinary release-main/query consumers as well
as R3. When no explicit browser deadline is supplied, derive a finite monotonic
deadline at fixture entry using the actual launcher runtime environment (1800
seconds for the default launcher). An explicit R3 deadline remains the earlier
activation-based global deadline. Import the supervisor before the loaded-source
census and always pass the held freshly-created browser-budget descriptor. The
outer launcher remains the hard whole-tree backstop; this grants no additional
service runtime. Source controls cover default1800, explicit deadline preservation
and invalid/missing runtime refusal. The actual #42 consumer smoke remains a
separate prerequisite; a direct supervisor control cannot substitute for it.


## Hosted defect-receipt cleanup binding correction

Prospective trust-boundary correction, 2026-10-16: hosted run 38843945807 at
`d537086` reached the unchanged API-cache defect assertion with exit 1, then
failed its receipt checker because that checker still required the removed
cleanup prose. Preserve that failed run. The minimized observed receipt lives
at `tools/ci/fixtures/browser-api-cache-d537.json` with its original digest.

Both browser defect graders must retain their existing exact defect, status,
flag and prerequisite checks. Replace only the obsolete cleanup substring with
the current exact cleanup label plus nonempty unique positive PID/start-tick
bindings, all kernel-empty and children-reaped observations, no stop error,
matching complete integer process exits and own-child ECHILD confirmation.
Reject absent/false observations, duplicate bindings and the historical prose
alone. Do not change the injected defect or its expected assertion. Test the
observed hosted receipt and representative corruptions before rerunning CI.
A corrected offline grade does not relabel the original failed workflow green.

## Failed-run evidence retention disposition

Trust-boundary amendment, 2026-10-18. Hosted receipt
`results/console-source-full-gates-01/pr82-third-fast-negative-browser-01.json`
has SHA-256
`b12ff444cac0d4f083f0d9d1b411485cbba781d38930b97fd391dacf7e392e37`. It records
the unchanged API-cache defect and prerequisites, with complete process stops,
but its cleanup disposition retains failed-run evidence in owned fixture
storage. The grader's prior exact cleanup string represented fixture removal and
therefore refused this valid retained-evidence receipt.

The browser defect graders admit exactly these two cleanup disposition values:

- `owned fixtures removed; process groups stopped and bound children reaped`
- `failure evidence retained in owned fixture storage; process groups stopped and bound children reaped`

The first records successful fixture removal; the second records intentional
failure-evidence retention. Either value still requires the existing cleanup
evidence: nonempty unique positive PID/start-tick bindings; every group
kernel-empty and children-reaped; no stop or cleanup error; a complete process
exit map with one exact integer exit per stopped group; and own-child ECHILD
confirmation. The exact defect name, exit status, injected-defect flag and
prerequisite checks remain unchanged. Every other cleanup label, including the
historical prose alone, remains invalid. Preserve and regress the hosted receipt
without changing its failed workflow outcome.

## Native WebDriver BiDi viewport calibration

Prospective geometry-only harness amendment, 2026-10-18. The classic WebDriver
`Set Window Rect` request for 390 by 844 CSS pixels produced an actual 500 pixel
`innerWidth` in the owned Firefox 157 fixture. In the observed fixture, the
independent Bun native-WebSocket reference used
`browsingContext.getTree` with `maxDepth: 0`, confirmed the classic window handle
equalled the one matching root BiDi context id, and used
`browsingContext.setViewport` with a 390 by 844 viewport. JavaScript then read
`innerWidth === 390` at DPR 1, both before and during CSS `zoom: 2`; the existing
18px control remained 18px. `viewport: null` followed by the classic desktop
restore returned the outer rectangle to 1440 by 1100 and JavaScript `innerWidth`
to 1440 (`innerHeight` was 1014). Twelve successful BiDi reply shapes were
observed with no unsolicited events in this finite run. The
raw actual run is `DATA/results/firefox-viewport-calibration-01/actual-terminal-01.json`,
SHA-256 `a0738f4c191ed298d2ff9f7a9e36e9ab7f21b967913ab821bfa02472b28292f3`;
its state records actual exit 0 and stopped cleanup 0 at source
`1ff56e6b79a03dc6ca7ef50498af988db95dd5e3`. This is one observed Firefox
geometry calibration, not product/browser acceptance or evidence for other
drivers.

The prospective native primitive is one narrow, existing-`xtask` command for
`browsingContext.setViewport`, with modes 390 by 844 and `viewport: null` for
restore. The legacy caller supplies its already observed WebSocket URL and
current Classic window handle. It may connect only to a literal `127.0.0.1` or
`[::1]` address with an explicit port and bounded driver session path; DNS,
`wss`, user information, query and fragment are refused. Before the viewport
command, `browsingContext.getTree(maxDepth: 0)` must return at most 32 root
contexts with unique ids and exactly one id equal to the supplied handle. A
missing match, duplicate id or malformed tree fails closed; the primitive must
not select an arbitrary context. Other uniquely identified root contexts are
allowed.

The primitive uses pinned synchronous Tungstenite 0.30.0 with the handshake
feature only, through an absolute-deadline socket adapter. One monotonic deadline
of at most five seconds covers connect, HTTP Upgrade, both BiDi commands and
their replies; per-read timeouts must not restart it. Preserve Tungstenite's
Upgrade protections: its handshake rejects after 65,536 input bytes or 512 read
packets, uses 4 KiB reads, and limits parsed headers to 124. Because the byte
check follows each read and appending, allow one further read chunk and allocator
capacity when describing this bound. The upgraded connection additionally has
a cumulative 2 MiB input budget, at most 32 received messages, 64 KiB
frame/message bounds, and small explicit WebSocket buffers. Unsolicited BiDi
events are rejected. Each command has one request id. Accept only its matching
success reply, with the command's expected result shape and no error; malformed,
unexpected or over-bound traffic fails closed. The client closes after that
response, so it makes no claim about later duplicate traffic.

The Rust command's bounded acknowledgement reports command/context and exit
metadata only; it does not claim the browser's actual viewport was verified.
The later Python caller must independently read `innerWidth` through classic
WebDriver and require exactly 390 before applying the existing 18px, 200% CSS
zoom and negative-overflow predicates. It must call the restore mode in a
`finally` path while preserving the original failure before cleanup, then restore
the original classic desktop rectangle. The Bun reference remains an independent
diagnostic and is not replaced by the Rust mock server or command.
