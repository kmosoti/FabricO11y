# Private assistive-technology environment and announcements

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
