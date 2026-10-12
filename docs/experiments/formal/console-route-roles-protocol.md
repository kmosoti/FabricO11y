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
