# Complete packaged route and role matrix

Prospective successor for C3 of the [console protocol](console-access-protocol.md),
registered before implementation or execution. Issue45 and C-027 require every
registered route as anonymous, Spindle, reader and owner. Accepted
[ADR-0029](../../decisions/ADR-0029-use-three-fixed-roles-for-the-first-release.md)
owns authorization; this protocol changes no role, quota, expiry, wire format,
resource limit or package byte.

The [literal fixture](fixtures/console-route-roles.json) states 36 method/action
rows and four explicit outcomes each: 144 cells. Expectations come from the
product contract and ADR, not `required_role` or a successful response. Coverage
must include both `console.rs` and its merged `http.rs`: the native30 omitted
legacy POST nodes, PUT config, POST pause/resume/revoke and POST query. Comparing
only templates also misses a lost action or a GET/POST distinction.

The production browser fixture must issue each cell exactly once with a
credential-explicit trusted TLS transport. Anonymous, Spindle and reader requests
carry no owner cookie. Owner mutations use the real browser's cookie, CSRF and
exact Origin. Public auth routes need that Origin for every caller. Pace all
dispatches with the shared fixture pacer, including both ceremony requests;
the table contains 16 auth-lane calls, at least 550 ms apart against 2/s burst 4.
Record actual counts/timings and additional setup calls. No hidden write retry
or 429 may satisfy an authorization rejection.

The table distinguishes authorization from domain validation: malformed public
ceremony finish returns 422; admitted malformed Spindle Batch returns 400;
unknown reader/passkey/session IDs return owner 403 `access denied`, while reader
access to those routes is 403 `role not permitted`. Public packaged shell assets
are 200 rather than the native empty-assets 404. These cases do not establish a
successful credential mutation; existing real browser workflows own that proof.
Intake/config refusals carry a `status` field; console/master-route refusals
carry `error`. The fixture binds that field rather than accepting either one.
Use the dedicated owned `matrix` node for successful config/pause/resume/revoke;
put its revoke and owner logout last. Owner freshness/session changes remain
real and visible. Preserve immutable canaries and the existing workflows.

Each observation records credential kind/method/template/path/status/refusal and
timing, never cookies, CSRF, bearer tokens, challenge material or returned secrets.
Check the full 144-cell set and multiplicities, both router method registries and
all three actions per node-control template. Require exact semantic refusal
messages where the fixture specifies one. The external record of actual dispatched
cells must agree with the expected set; declaring a table without running it is
insufficient acceptance.

The deterministic red regression executes only the existing selected-matrix AST
with synthetic request callbacks, recording its offered cells; it never invokes
`main`, browser, network, process or native service. The successor callable
`run_role_matrix(request, check)` must offer all 144 cells through that same narrow
transport interface. Reader-admitted-as-200 is a named checker defect, not an
arbitrary failed process. Literal controls also exclude 429/wrong error and show
that dropping legacy resume is missed by template-only coverage but not action
coverage. Keep failures; do not edit expectations to match incomplete calls.

Root runs the red before implementation through the resource launcher:

```sh
python3 -B tools/resource_group.py -- python3 -B -m unittest discover -s tools/ui -p test_role_matrix.py -v
```

Then root repeats the unchanged command on the implementation revision. These
mocked checks support completeness and the checker, not package acceptance.
Actual Chrome/Firefox cells additionally require new frozen package/console
hashes, fresh #41, clean exact merged harness, default 1,800-second containment,
the existing stricter 4 GB browser fixture, storage admission, stopped descendants
and secret/scratch cleanup. Publish exact candidate-bound argv, exit status,
fixture hash, full public observations and cleanup before crediting C3. All
new checks and package matrix cells are **not run** at registration.

## Separate finite matrix cell

Run each pinned desktop engine with the existing required build/server/Spindle,
packaged-console and fresh output arguments plus `--role-matrix`. This prospective
mode reuses the real TLS/resident-key/reader/native-producer setup and existing
cleanup. It ends after all 144 cells, the actual owner-logout UI lock, response
headers, artifact identities and dispatch evidence; it does not claim the later
baseline workflows completed. Default runs retain their selected-role probes and
all original workflows, but those probes do not alone satisfy complete C3.

Reject matrix mode combined with nonzero polling, pressure, expired-cursor,
recovery or shell-lifecycle options. Those remain separate acceptance cells. The
144 dispatches take at least about79 seconds at the registered cadence; combining
them with two-minute polling could exceed the unchanged240-second inner deadline.
Keep the default1800-second outer deadline,4GB cap,no swap,CPU200%,tasks512 and
512MiB fixture ceiling. No expiry or performance limit changes.

The shared cadence's interval also drives a serialized browser fetch scheduler,
so automatic WebAuthn start and finish are separately paced. The already
registered sixteen-request overload diagnostic is explicitly unpaced, recorded
as such and still must observe its exact429. Owner CSRF support retains the
deliberate missing-CSRF logout as an explicit opt-out negative probe. All matrix
requests use the existing trusted TLS helper, disable redirect following, keep
bearer/public contexts free of owner cookies and record bounded response metadata.
Before the matrix, perform real passkey reverification; verify the browser's new
session identity, and use its matching real CSRF. Revoke the dedicated matrix
node and log out only at the final rows. Record actual browser setup/auth counts
and timestamps separately from the literal144 matrix cells/16 auth dispatches.
The pinned setup offers seven browser auth dispatches: rejected initial start1,
owner registration2, rejected consumed-bootstrap start1, additional-key finish1
and pre-matrix login2. Require that literal count and at least550ms between those
recorded starts, including each automatic finish. These seven precede the matrix's
sixteen public auth dispatches; the already registered private peer burst remains
an explicitly separate sixteen-request unpaced diagnostic.

The inner deadline and512MiB storage event are dispatch gates, not only the
final UI wait. Check before and after pacing, before opening either native TLS
or WebDriver transport and after each bounded body chunk/response. Socket waits
must be positive and at most the remaining inner deadline (also at most their
existing15s/35s ceilings); refresh the receive timeout as that budget shrinks.
A continuously progressing body must not evade the wall bound through an idle
socket timeout. The independent review of0de4b46 supplies literal expired,
storage-limited, paced-expiry, remaining-time and progressive-body fixtures;
mock only transports/time, retain actual source dispatch functions, and require
refusal before network or before the next body chunk. Root records named red
then repeats unchanged controls after the separate implementation fix.

## Absolute blocking-I/O control

CPython reads HTTP header and chunk-extension lines inside urllib/HTTPResponse;
even `read1` can perform multiple internal receives for that metadata. An idle
socket timeout plus checks between returned body chunks therefore does not alone
enforce the absolute inner deadline. Register a separate control before adding
the remaining guard: the actual extracted WebDriver dispatch contacts only an
owned loopback synthetic peer, which sends a byte every10ms within a header or
chunk-extension line for0.6s. The two deadline cases use a literal0.12s remaining
deadline and require the named deadline refusal within0.35s. A third case sets the
existing storage Event after0.12s inside a progressing header, uses a2s deadline
to distinguish the cause, and requires the named storage refusal within0.35s.
Restore the previous SIGALRM handler/timer and stop/join the peer after every case.

The prospective enforcement is a shared main-thread per-I/O SIGALRM guard, armed
from the unchanged remaining deadline and polling the existing storage Event at
most every100ms. A raising handler must interrupt internal blocking reads rather
than allowing continuous progress to restart an idle timeout. Preserve genuine
transport exceptions, restore/cancel signal state on every exit and before
fixture cleanup, and reject incompatible signal ownership rather than overwriting
another timer. No host clock,240s/1200s inner deadline,1800s outer deadline,quota
or admission threshold changes. Root runs the same contained unittest command
above for named red then unchanged green; all three actual-I/O cases are **not
run** at registration and establish harness bounds, not package acceptance.

## Actual browser dispatch timing control

Miso's independent counterexample distinguishes a resolved pacing reservation
from the later actual fetch continuation: if the first continuation takes5ms and
the second does not, reservations550ms apart can yield starts only545ms apart.
Keep the shared550ms threshold. Before changing the scheduler, run its exact
injected JavaScript through the existing Bun runtime with synthetic fetch,
monotonic clock and timers; no browser, network or product process is invoked.
Delay only the first continuation after pacing resolution by a literal5ms.
Record actual fake fetch calls independently of the scheduler's receipt, require
their timestamps to agree, and require the two paced starts at least550ms apart.
The existing scheduler is predicted to offer starts5ms/550ms; root must record
the actual named red rather than treat this deduction as executed evidence.

The control also offers the already registered sixteen-request unpaced burst,
requiring sixteen same-clock starts marked unpaced, with no lost dispatches,
retries or altered response statuses. Fake200 responses test scheduler behavior,
not server admission. The prospective fix must record and advance the shared
clock at actual dispatch within the serialized pacing slot, preserving automatic
ceremony cadence, explicit reservation users, exception visibility and the
separate overload diagnostic. Root runs the same contained unittest command
above, then repeats the unchanged literal control after a separate implementation
commit. The Bun child has a5s synthetic-control limit inside default containment;
all browser240s/1200s limits and quotas remain unchanged. This control is **not
run** at registration and does not credit packaged browser acceptance.
