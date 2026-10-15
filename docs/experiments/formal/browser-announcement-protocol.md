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
