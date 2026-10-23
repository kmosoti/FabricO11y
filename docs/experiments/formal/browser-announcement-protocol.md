# Private assistive-technology environment and announcements

## Packaged validation-alert counterexample

Registered before correction, 2026-10-23. Exact product `e4d51bd`, package
snapshot `62b4458`, harness `42bc301`, diagnostic04 retained real status speech
and PCM growth. Its validation alert was visible and had `role=alert`, but its
AT attributes lacked assertive live metadata; the action produced no matching
speech and no PCM growth. The unchanged alert oracle refused
`announcement-synthesis`. Preserve this release blocker and stopped custody.

The literal [counterexample](fixtures/console-alert-presentation.json) records
the action and missing metadata. Add a cheap actual-DOM assertion to the existing
invalid-filter browser action: its alert must retain the expected text and have
explicit `role=alert`, `aria-live=assertive` and `aria-atomic=true`. This supplements
the existing status-role assertion; it does not replace actual AT events, accepted
speech or synthesis. `test_browser_alert.py` must reject the old role-only state,
missing/changed fields and a different error while accepting the literal valid
metadata. Source controls do not establish native presentation.

Make the existing conditional validation alert explicitly assertive and atomic;
keep the original absent-before-error baseline, text, input limits and behavior.
No API, authorization, telemetry or session change is permitted. This product
fix requires a successor package candidate and fresh affected gates. Keep the
independent alert/event/speech/PCM oracle and suppressed-announcement expectations
unchanged. A successful source/DOM check cannot admit the successor before its
real packaged positive and suppressed controls complete.

## Already loaded document counterexample

Registered before correction, 2026-10-23. The engineering AT diagnostic on
product `84a585d` heard the actual Check session focus presentation but refused
`announcement-document-identity`. Its77 recorded events contained only parent
and name changes, with no document-load event. The initial desktop was empty;
the document could already be loaded when observation began. Preserve the
original exit1 and stopped fixture custody. This does not yet test the repaired
validation alert and does not justify another product candidate.

Extend the existing document lifecycle regression with an initially empty
desktop and a later actual focused-state event from the browser application
containing an already loaded document. Separately traverse that application's
AT tree with the existing2048-node/five-second bounds and emit only the existing
empty-text identity snapshot. Focus loss and unrelated property events must not
trigger that traversal. Wait within the existing caller deadline for the exact
browser PID/DocURL snapshot after actual focus speech. Keep every announcement
event, SSIP225, PCM, negative outcome, stream limit and cleanup predicate. This
is an identity observation trigger; focus speech or snapshots cannot replace
live-region presentation evidence. Reproduce the source failure before repair,
then rerun affected controls and fresh packaged positive/suppressed AT cells.

## Late document registration counterexample

Registered before correction, 2026-10-23. At `57383c4`, the unmerged packaged
announcement diagnostic retained real reader focus speech, then refused
`announcement-document-identity`. Its initial desktop snapshot contained no
document rows. Later actual `document:load-complete` events independently
identified the console URL and browser PID77. Preserve the stopped failed
diagnostic; neither focus speech alone nor an unbound event grants acceptance.

Register `test_announcement_document_lifecycle.py`: an initially empty desktop
later registers a real document-load source. The observer must separately query
that AT document's identity after the load, preserving the actual event and
emitting the existing identity-only snapshot with empty text. A nondocument
event must not create a document snapshot. Keep the original snapshot traversal
bounds and PID/DocURL matching; no event may be relabeled as a snapshot or vice
versa. Snapshots still cannot substitute for actual live-region presentation
events, SSIP225 speech and PCM growth. Keep setup-secret clearance, stream
bounds, deadlines, cleanup and every existing refusal unchanged. Run the source
counterexample before repair, affected controls after repair, then fresh positive
and suppressed packaged announcement rows on the merged harness.

## Owned speech-daemon lifetime counterexample

Registered before correction, 2026-10-23. Fresh packaged announcement row69 at
`4dc0022` stopped before owner enrollment. The private observer reported
`announcement-process-exit:speech-dispatcher-process`; that daemon had exited0
before controller stop. The observer then stopped the private driver/display,
and the outer browser reported a Marionette transport failure and failed
announcement cleanup. Keep every original failure and semantic cleanup error.
The original resource receipt and owned subgroup observations confirm kernel
shutdown and reaping; preserving those files does not grant acceptance.

A separate private native lifecycle diagnostic with the exact installed speech
daemon observed default idle exit0 after5.863 seconds. A configuration adding
`Timeout 0` remained running after7.033 seconds, then stopped with exit0 on
the controller's explicit TERM. No Orca, product or announcement acceptance
follows from this diagnostic.

Register `test_announcement_lifetime.py` before the producer correction. The
old generated configuration must fail the requirement for exactly one explicit
`Timeout 0` line. Preserve the same real espeak-ng module, ALSA file capture,
log bounds and private directories. Disable only this daemon's internal idle
exit in the disposable configuration: the original environment/caller deadline,
resource limits, bounded stop/reap behavior and persistent-process exit refusal
remain mandatory. No keepalive speech, fake utterance, dummy backend or bypass
of successful SSIP225/PCM/AT-SPI evidence is allowed. Run the source counterexample,
affected source controls and fresh positive/suppressed packaged announcement
rows after merge. Host audio, display, service and settings remain untouched.

Prospective prerequisite for issue45 C7/U5; **not run**. This environment probe
uses only public fixed live-region text. It supplies no packaged-console or
accessibility-conformance acceptance. Exact candidate actions must later reuse
this observer after setup secrets have left the DOM.

Run under the unchanged resource launcher and shared scheduler, with one GiB
whole-owned reservation, DATA-only scratch, 2GiB memory max/1GiB high, no swap,
CPU200%, tasks256 and an inner180-second deadline. Bubblewrap supplies a private
PID/network/IPC namespace, DATA-backed `/tmp` and private runtime/config/cache/
data directories; no owner display, bus, settings, audio or trust is reused.
The loopback interface belongs only to that private network namespace. A private
`dbus-run-session` owns AT-SPI. If selected, verified private Xvfb uses only its
private UNIX socket with a private Xauthority cookie and TCP disabled. A headless
probe may establish absence; preserve that result before a headful successor.

The observer retains actual AT-SPI events, source process ID/role/attributes/text
and monotonic observation times. A private Orca customization transparently wraps
its existing Speech Dispatcher `speak` call: invoke the original function with
unchanged arguments, retain its actual successful SSIP225 message ID, selected
`espeak-ng` module and generated utterance, then return the unchanged result.
An owned capture marker gates speech collection; absence means no utterance text
is recorded. This is an observation hook in the real Orca speech path, not a
replacement synthesizer or a direct test call to Orca's presentation API.

Speech Dispatcher uses its real espeak-ng backend and private ALSA file/null
output. Require the actual backend process/executable/libespeak mapping, selected
module and nonempty PCM output created during the action window. The public
probe changes a polite status while focus is on another control. Require its
actual document/AT-SPI event and matching accepted Orca utterance in the same
bounded action interval, after the original DOM role/live prerequisites.
This establishes generated speech/synthesis, not a listener or acoustic test.
Orca `SPEECH OUTPUT` debug text alone cannot satisfy the checker.

The independent checker must reject missing speech, missing/wrong-process AT-SPI
events, wrong backend, missing/empty synthesis and out-of-window evidence. The
native suppression control sets `aria-live=off` on the same unfocused status
only after observing its original role/live prerequisites. It must fail the
announcement assertion, not startup or DOM prerequisites. Preserve unexpected
suppression behavior as inconclusive; never fabricate silence.

Bound each event/speech record at4KiB, each raw stream at1MiB and all owned
fixtures at512MiB. Stop/reap the private namespace and all descendants before
cleanup; keep failed public evidence and uncertain scratch. Record exact source,
tool/config hashes, argv, versions, namespaces, process IDs, resource observations,
actual command exits and cleanup. Xvfb acquisition is separately registered by
X218; a prepared tool is not a working AT environment or packaged-browser result.

## Private loopback prerequisite correction

The first headless native probe retained `ip link set lo up` exit2 with
`RTNETLINK ... Operation not permitted`; no accessibility step ran. A separately
contained same-topology diagnostic at `f90c4be` exited0: private loopback flags73
included IFF_UP, the network namespace differed from the owner namespace, and
an owned 127.0.0.1 socket delivered exact fixed bytes. Its Python process retained
CAP_NET_ADMIN, so capability loss is not an established cause of the ip refusal.
The unnecessary interface mutation is removed. Require the actual private
IFF_UP flag and exact owned loopback delivery instead; never change an owner
interface. All AT-SPI/Orca/espeak requirements remain unchanged. Successor runs
use a fresh output-derived scratch directory, preserving the failed first run.

The corrected headless02 run retained a second prerequisite failure: Speech
Dispatcher's level3 voice-inventory diagnostics exceeded the unchanged1MiB
stream bound before browser/AT actions. Its actual loopback witness succeeded.
The successor uses level2 diagnostics and distinct daemon/process output files;
module identity, real SSIP acknowledgement, PCM output and every existing bound
remain mandatory. Original logs, exits and namespace stop receipts are retained.

## Native diagnostic stream allowance correction

Headless02/03 retained complete Speech Dispatcher startup inventories of
1,370,173 and1,369,137 bytes respectively. Upstream0.12.1 emits those legitimate
voice rows at log level1. The owner explicitly selected preservation of the full
installed voice inventory and errors, rather than masking tool data or disabling
logging. No voice-data masking was executed; that prospective proposal is
withdrawn. Preserve the existing synthesizer configuration and voice tree.

Prospectively allow only the daemon's exact
`speech-logs/speech-dispatcher.log` up to4MiB inclusive; every other stream retains
1MiB and records retain4KiB. The aggregate512MiB owned-fixture ceiling and1GiB
reservation remain unchanged, as do launcher limits and every AT-SPI, Orca,
SSIP225, actual module/library and PCM assertion. Nonzero exits remain errors.
A literal boundary control must accept4MiB, reject4MiB+1 for that exact daemon
log, and reject1MiB+1 for any other stream. This corrects the undersized auxiliary
diagnostic allowance, not a product or semantic acceptance gate.

The output-derived private runtime backing path can be103 bytes before AT-SPI
appends its socket suffix. Use the existing private bind at `/run/user/UID` as
`XDG_RUNTIME_DIR` and the speech-socket alias, keeping UNIX socket names within
kernel limits. The inner process checks that this alias has exactly the owned
DATA-backed runtime directory device/inode. This does not expose the owner bus;
the private bind continues to hide the owner runtime path.

Headless05 reached the private AT bus after retaining the full voice inventory,
but `--clearenv` omitted `FABRIC_STORAGE_ROOT`: the inner imported storage default
then pointed to its private XDG cache, and geckodriver failed with ENOENT. Pass
the admitted DATA storage root and owned scratch root explicitly into the private
environment. Preserve the original missing-tool failure; it is not an AT
announcement refusal or a display limitation.

The actual headless06 probe reached the real Firefox document and synthesizer,
but Orca exited1 because no Desktop/display was available. The signed-Xvfb01
successor then aborted during optional GLX initialization in the NVIDIA EGL/libdrm
path; its complete backtrace and stop receipt remain preserved. For this CPU
virtual text/announcement fixture, disable optional GLX with the tool's supported
`-extension GLX` flag, precreate only the private DATA-backed X socket directory,
and require the display process to remain alive during readiness. No GPU/3D
coverage is claimed; no owner graphics configuration or display is changed.
Every actual event, Orca speech, selected backend and PCM assertion remains.

Xvfb02 retained a working private headful Firefox document and51,102 startup PCM
bytes, but the AT registry never activated: the installed AT bus broker requested
`org.freedesktop.systemd1` on the private session bus and reported a failed registry
unit. The isolated fixture deliberately has no owner session manager. Start the
installed real `at-spi2-registryd` explicitly on the freshly returned private AT
bus address, without GNOME session registration, before Orca/browser startup.
Require the observer to obtain a genuine AT desktop before writing its readiness
marker. This changes service startup ownership only; no event, speech or synthesis
is supplied by the harness, and all action-window assertions remain unchanged.

Xvfb03 obtained genuine registry events for the target status and actual PCM
growth, but retained `announcement-utterance`: Orca was still presenting the
initial document and rejected the immediate live update as a short-time event
duplicate. Before changing the status, require actual successful Orca speech for
the initial public document, then refocus the existing public button and require
its real utterance. SSML markup in those setup utterances is parsed as XML text
only to establish setup readiness. The target action still uses the original
eight-second window inside the unchanged overall deadline, with every original
event, utterance and synthesis assertion. Setup observations never satisfy the
target window and no direct Orca presentation call is made.

Xvfb04 failed before the target action: accepted document SSIP225 was observed,
then Orca exited with `RuntimeError: can't start new thread` in its genuine speech
health-check thread; Firefox content processes also crashed. The initial timing
hypothesis alone does not explain that failure. Memory remained below1GiB with
no OOM; the256-task ceiling is a hypothesis because task-event counters were not
recorded. Prospectively retain the256-task limit and all existing gates/windows,
but restrict the owned probe and descendants to allowed CPUs2-3, within the same
200% quota, to bound tool thread pools. Record actual affinity and cgroup
`pids.current`, `pids.peak`, `pids.events` before launch, at critical inner phases
and on completion. This discriminates task-admission pressure without raising a
resource ceiling or changing any announcement expectation. Preserve the crash.

## Faithful SSML observation join

The retained Xvfb04 startup SSIP225 record contains the public title as an XML
`speak` element with empty interword `mark` elements. Literal raw substring
matching therefore rejects the actual spoken title despite preserving every
word. The minimized format fixture records the original raw row, source hash,
revision and public-only scope; it is not a target announcement pass.

Prospectively parse only a plain utterance or an XML `speak` root without
attributes containing empty `mark` children with only the observed numeric
`name` offsets. Join XML text/tails in order without case, whitespace, punctuation
or word normalization. Keep the raw SSIP utterance. Reject malformed XML, DTDs,
processing instructions, unsupported roots/elements/attributes, attribute-only
expected text and wrong spoken words. Existing literal controls, actual event
PID/action times, exact expected words, SSIP225/message ID, selected backend and
real PCM growth remain mandatory. This is an oracle trust-boundary correction
registered separately from implementation; no direct substring relaxation.

## Measured private fixture task allowance

The CPU2-3 Xvfb05 probe recorded task peak256 and nine refused task admissions;
at browser readiness `pids.current` was252. Orca heard the initial document and
focus control, but Firefox content crashed before the target action. The owner
authorizes a prospective512-task internal fixture ceiling, matching the scoped
browser envelope. Keep2GiB memory, no swap, CPU2-3/200%, all deadlines, storage
bounds and every semantic oracle. The whole-project launcher's memory/runtime
limits do not change. Retain the original refusal/crash and task observations.
Require no task-admission refusals in a successful successor and treat an exited
persistent display, bus registry, reader, synthesizer or driver as an environment
failure, preserving actual exits. No failing startup may count as suppression.

## Public document URI control

Xvfb06 kept task peak342/max0 and the browser alive, with accepted document and
focus utterances. The target emitted actual AT events, but no target utterance
or PCM growth. Orca reported that the data-URI document had no AT document URI;
its debug queue then stopped at a stale New Tab load/cache event. Prospectively
serve the identical public HTML bytes through an owned HTTP server inside the
existing private loopback namespace, leaving browser preferences unchanged.
Record actual response bytes/hash and the real AT document URL for the fixture
browser before the target action. This isolates document URI behavior; keep the
512 tasks, CPU2-3/200%,2GiB, eight-second action window and every existing oracle.
Preserve the no-URI result and never substitute setup speech for target speech.

## Official corrected reader admission

Xvfb07 retained the HTTP/AT document URL, real status events and PCM growth, but
Orca50.3 classified the status-bar insertion as UI_UPDATE before testing live
regions and omitted its utterance. Upstream's exact correction is
[2f8d804](https://github.com/GNOME/orca/commit/2f8d804ca2998d2cee7a9e81b61e7bcd238285b0),
with an independent native status-role regression. Use the unmodified official
[Orca51.1 release](https://download.gnome.org/sources/orca/51/) as a separate tool
input, preserving the50.3 limitation. Its signed Fedora noarch package is privately
extracted; corrected source files must equal the checksum-pinned upstream archive.
No host installation, presentation patch, DOM change or oracle relaxation occurs.

Before native use, bind the prepared receipt to the entire regular-file inventory,
launcher, private Python source path and private compiled schemas; reject changed,
missing or extra files and symlinks in executable source/schema paths. Launch its
unmodified script with installed Python3.14, within upstream's supported minimum.
Require the actual loaded Orca version/module path from the private customization
before browser actions. Retain the identical status page/eight-second action
window/event→Orca→SSIP→module/PCM assertions and all resource limits. Publish exact
prepared hashes in a separate native pre-flight; tool preparation alone is no
announcement or package acceptance evidence.

## Actual console announcement caller

The isolated announcement cell uses the exact packaged server, Spindle and console
assets admitted by the ordinary caller. It excludes migration, expiry, security,
response injection, roles, polling, pressure, cursor, recovery, shell and unrelated
negative-control modes. Firefox157.0.1 remains explicit; no silent engine change.
The controller/server retain their existing ownership/cgroups. Only the owned
WebDriver/browser/display/AT subtree enters private PID/mount/IPC namespaces,
private DATA-backed `/tmp`, runtime/config/cache/data, authenticated Xvfb and DBus.
It shares the existing fixture network so the actual TLS localhost server remains
reachable. This differs explicitly from the public environment probe's private
network. The combined4GB/512-task/CPU200%/240-second/storage limits remain; restrict
the AT subtree to CPUs2-3. Register exact topology/tools/source before native use.

Start the private observer/Orca only after genuine owner registration, bootstrap
secret removal and positive CSRF/Origin controls; reject residual secret fields.
Use owned file markers solely for start/stop/capture coordination, never to supply
events or utterances. Join actual Firefox namespace PID, AT document URL, events,
real Orca SSIP225/module and PCM growth. Every action has its own bounded eight-
second observation interval within the original shared240-second deadline.

Observe actual status `Working…` and completed owner status during Check session,
holding only the already received real server response for1second so busy remains
observable; keep actual fetch arguments/response and request pacing unchanged.
Observe the real4097-byte filter validation alert, logout/locked status, keyboard
Tab/Enter authentication with the real virtual resident/UV key and native keyboard
skip-link activation. Alert DOM may rely on its implicit assertive live behavior;
require genuine AT alert/assertive attributes as well as actual speech/synthesis.
The status checker retains its exact existing role/polite/focus assertions.
Suppress only the actual status's `aria-live` after its original DOM prerequisites;
its expected utterance must then be absent at the announcement join. A startup,
DOM, transport, resource or cleanup failure is inconclusive for that control.

Inventory every rendered actionable button/input/select/textarea/summary/link in
the finite Explore, Settings and locked/auth views at1440 and375 CSS pixels. The
target criterion is24×24 CSS pixels, or the WCAG2.2 spacing exception (a24-pixel
circle at each undersized target's center intersects no other target or another
undersized target's circle), or a literal inline text-link exception. Preserve
every rectangle, target text/type and exemption reason; a single button cannot
stand in for the inventory. This is finite desktop/narrow CSS coverage, not
physical-device compatibility or general WCAG certification. Literal geometric
overlap and missing/wrong alert/event/speech controls must be rejected.

Stop the interactive AT subtree before final cgroup teardown and retain actual
exit/namespace/cleanup receipts. The private native environment positive and
successful exact replacement installation remain prerequisites for package cells.

The geometry checker follows the finite size/spacing model from
[W3C's target-size explanation](https://www.w3.org/WAI/WCAG22/Understanding/target-size-minimum.html).
For axis-aligned rounded rectangles it tests whether a centered24-pixel square
fits the actual corner radii, including percentage radii and CSS radius scaling;
a24-pixel circle's bounding box alone does not satisfy the square requirement.
Transforms or clip paths outside that finite geometry model fail explicitly.
The literal inline exception requires an inline anchor within surrounding text,
not an arbitrary button or a paragraph containing only a standalone link.

The interactive reader starts after authentication. Record its initial accessible
document as an explicitly named `document:snapshot`, including the genuine AT
document URL and Firefox namespace PID. This bounded tree inspection is an
identity witness, never an event credited by an announcement oracle. Keep actual
object events and utterances as the independent action evidence. Configure only
the private reader's supported `say-all-on-load=false` setting, avoiding automatic
whole-page narration while retaining normal focus and live-region presentation.
The public environment probe keeps its original reader configuration.

Keep action summaries within the existing4KiB record bound; raw events and SSIP
rows remain in their original complete streams, referenced by action windows.
Stop actions at the original240-second deadline. Dependency cleanup has a separate
bounded25-second grace, never a new measurement window; any failed actual wait or
unexpected process exit fails the caller, followed by authoritative cgroup cleanup.

## Dependency-ordered reader cleanup

The retained Xvfb07 shutdown showed Orca exit1 after its display and registry
received concurrent termination. The probe's old final verdict preceded that
cleanup outcome. Before the prospective51.1 native cell, stop and wait for each
owned process in reverse launch order so driver/browser and Orca finish while
their display, registry and synthesizer remain alive. Record before/after exits,
signals, forced kills and actual waits. Each wait is bounded at2seconds, with
at most25seconds total cleanup within the existing outer180-second bound; do not
extend or reset the action/measurement deadline. An unexpectedly exited persistent
process, positive nonzero exit, forced kill or failed stop makes the final probe
verdict fail. Preserve its original action error separately. Literal lifecycle
controls must reject the observed premature-dependency/false-credit mechanism.
The actual console caller shares this lifecycle rather than copying a looser one.

## Complete synthesis diagnostic allowance

The retained HTTP Xvfb07 probe wrote1,019,394 PCM bytes before any accepted target
utterance, leaving only29,182 bytes within the original1MiB raw-file allowance.
The actual caller requires several additional utterances. Prospectively allow
only the exact `speech.raw` diagnostic file up to8MiB inclusive for the public
probe and caller. Preserve every byte; do not truncate, rotate, discard or split
output to bypass a bound. No sample rate or audio duration is inferred here.
Require8MiB acceptance/8MiB+1 rejection controls, while every other raw stream
remains1MiB and the daemon's previously registered exact log remains4MiB. Keep
records4KiB, aggregate512MiB, reservation1GiB, cgroups/deadlines and all independent
AT events, accepted utterance, exact backend and PCM-growth assertions unchanged.
This is a bounded auxiliary diagnostic allowance, not a semantic/product gate.
