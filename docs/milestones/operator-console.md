# Milestone: authenticated operator console

Status: **integrated implementation in verification; release acceptance incomplete.**
Owner direction: 2026-10-10. A Rust Leptos WebAssembly UI, installable as a PWA,
is part of the [bounded release](release-readiness.md), not a later milestone.
The reference images describe the visual direction; their numbers and controls
are not evidence of implemented behavior. Authentication and authorization follow
the [identity and access plan](../architecture/identity-access.md); local passkeys are the confirmed
human login method, with OIDC integration deferred.

The [UI crate](../../crates/fabric-ui/README.md) provides native-testable
request/tail/chart/polling models and live Leptos passkey, query and control flows.
The server serves hash-verified assets and enforces scoped authorization. The
[algorithm model](../architecture/console-algorithms.md) owns mathematical bounds;
the [art-direction flow](../architecture/console-art-direction.md) connects
operator tasks, evidence, visual encoding and evaluation. A demonstration does
not fill a packaged authenticated release gate.

## Boundary and present evidence

Use Leptos client-side rendering, built into static HTML, JS, CSS and WASM assets
and served by the production Axum server under the same HTTPS origin as the API.
This matches [Leptos CSR deployment](https://book.leptos.dev/deployment/csr.html).
Pin Leptos, the Rust/WASM toolchain and asset builder before implementation.
SSR adds no required capability for this authenticated console; do not introduce
a second server, CDN runtime, or separate origin solely to render it.

The [console HTTP adapter](../../crates/fabric-server/src/console.rs) exposes
`/v1/console` behind scoped credentials and serves `/console/`. Production `serve`
requires passkey configuration and denies legacy `/v1/admin` routes. The
[query contract](../architecture/retained-history.md) and
[query implementation](../../crates/fabric-server/src/query.rs) establish actual
filters, pagination and evidence metadata. The independent query/rate oracles
remain authoritative; a UI adapter must not silently change their semantics.

Keep versioned wire DTOs and serialization outside the dependency-free pure
core. Register the client/API compatibility boundary and layer assignments before
adding crates. The client calls authorized effect adapters; browser state never
becomes a trusted authorization decision or a storage/delivery state machine.
Missing APIs below are release implementation gaps, not promises already met.

## Visual brief and required workflows

Use the FabricO11y cyan brand with white surfaces in light mode and deep navy
surfaces in dark mode. Preserve readable density, restrained cards, clear chart
axes, source labels and visibly distinct uncertainty states. Desktop navigation
uses Home, Explore, Traces, Pipeline and Settings. Mobile uses a bottom bar for
Home, Explore, Traces, Pipeline and an investigation/status destination; retain
the reference Alerts label only if its purpose is clearly explained without
implying an implemented alert evaluator. Settings remains reachable on mobile.

| Screen | Required first-release behavior |
| --- | --- |
| Overview | Real server/companion process observations, per-source freshness, retained window, gaps and completeness; bounded links into matching evidence. Distinguish measurement time, query time and source clock assumptions. |
| Explore | Logs, metrics and spans tabs; explicit half-open time range; source selector; supported filters; bounded rows/points and snapshot-aware next-page controls. Every answer keeps evidence metadata visible. |
| Trace detail | Search by exact trace ID; waterfall from actual span timestamps; span details, source identity and missing-parent indication. Link to related logs only where available evidence supports correlation. |
| Live Tail | Bounded near-live polling of logs, explicit poll interval, pause/resume, visible row cap and last successful refresh. Display poll gaps and retention expiry; do not claim lossless streaming. |
| Pipeline | Actual Spindle Spool → authenticated delivery → committed journal → bounded sealer → published Parquet Segments → journal/Segment query path. Explain custody and expose measured, defined counters. |
| Settings | Current identity/permissions, passkey enrollment/recovery and session revocation; existing Spindle enrollment, configuration, pause/resume and revoke workflows under scoped authorization; appearance, installation/help and read-only effective retention/storage policy with file-backed operator instructions. |

Log search supports source, time and exact case-sensitive body substring.
Metric history supports source, exact metric name and time. Span search supports
source, time, exact trace ID and exact name. Reject unsupported SQL-like syntax,
service grouping or arbitrary aggregation explicitly; do not reinterpret a DSL
as substring search. Resource/service attributes are not currently projected by
the registered queries. Nanosecond timestamps and integer counters must retain
exact values through wire decoding and display; JS floating-point conversion
must not round identifiers, times or integer values silently.

Metric charts consume a bounded point result and label truncation/pagination;
do not conceal reset markers, merge distinct attribute series, interpolate across
missing coverage, or turn partial history into a full-window aggregate. The current
rate response is unpaginated. Before enabling a rate chart, admit and independently
verify a server-side bounded result contract; a time selector or post-download
client truncation alone does not establish a response/memory bound.

The current log projection has body and string attributes, but no dedicated trace
ID field/filter. Related-log views may use an explicit supported body query or
inspect an already bounded result's declared trace attribute, labeling the scope
and limitations. No relation found means no matching evidence in that query,
not proof that the trace has no logs. Preserve duplicate spans, unknown parents,
partial traces and contradictory timestamps rather than fabricate a complete tree.

Pipeline/overview metric definitions must declare producer, unit, sampling window,
reset behavior and unavailable/stale behavior. Use the existing self-observation
path where sufficient; add a narrowly scoped authorized status API for missing
counters. Distinguish accepted Batches, committed records, projected rows, bytes,
backlog and successfully ACKed custody. A stale observation is never a live rate.

The reference's 124K events/s, 86 ms p95, SQL-style count-by-service, Arrow Buffer
stage and alert badges are illustrative. They do not describe a measured release
or establish an alert engine, APM percentile aggregation or a new pipeline stage.
Alert rules, notification delivery, background push and saved investigations need
separate explicit scope; the images alone do not authorize those subsystems.

Identity/access management and the existing Spindle control workflows are required
release screens. Missing human-auth management APIs are blockers; do not treat
all Settings as optional. An editable retention/storage screen additionally needs a scoped control API,
write authorization, validation, protected atomic configuration persistence,
desired/applied state, restart behavior and independent retention regressions.
It must explain whole-Segment deletion, irreversible expiry and separate journal,
workspace/Spool headroom. Do not expose a cosmetic control that changes only
browser state; read-only policy inspection is the required initial screen.

## PWA, privacy and lifecycle contract

Provide a manifest with stable identity, name, start URL, scope, standalone display,
theme/background colors and normal/maskable icons. Service workers require a
secure context; production uses operator-trusted HTTPS, with localhost reserved
for development ([MDN service workers](https://developer.mozilla.org/en-US/docs/Web/API/Service_Worker_API/Using_Service_Workers)).
Package the complete static asset set in both release packages and hash it with
the candidate; no runtime network fetch to third-party assets is required.

Cache only an explicit allowlist of versioned public app-shell assets. The service
worker passes API/auth requests directly to the network and never caches their
responses, headers or bodies. Exclude credentials, logs, metrics, spans, query
results and personal information from CacheStorage, IndexedDB, localStorage and
service-worker messages. No offline command queue, sync replay or persistent
last-result store is part of this release. A generic cache-all fetch handler is
forbidden by this console contract.

Offline startup may display the public shell and an honest connection-required
screen. Keep last-success timestamps/results only in authenticated page memory
while online. On detected network loss or an unconfirmed session, lock the data
view and clear displayed sensitive results; revalidate online before redisplay.
Logout/account changes clear memory, outstanding responses and cross-tab state.
Remote revocation cannot be discovered while disconnected; do not promise an
instant offline revocation guarantee. No stale offline view grants access.

Specify CSP, script/WASM policy, MIME types, HTTPS trust setup, anti-framing,
service-worker scope and response cache headers in packaging/operator guidance.
Sensitive responses use `Cache-Control: no-store`; the explicit worker allowlist
is independently enforced. Asset caching must never depend on an authenticated
HTML response containing credentials or embedded telemetry.

Use content-addressed assets and a versioned app-shell cache. Register API/client
compatibility and bounded retirement rules before shipping; detect an unsupported
version and show an update-required screen rather than issue incompatible writes.
Prompt for updates, finish/discard transient queries, reload a coherent asset set
and remove retired public caches. A worker update must not mix incompatible JS
and WASM. Test rolling upgrade, rollback, open tabs and outage during activation;
[MDN's lifecycle guidance](https://developer.mozilla.org/en-US/docs/Web/API/Service_Worker_API/Using_Service_Workers)
describes install/activate behavior, not proof of this implementation's safety.

Register exact OS/browser versions for desktop Chromium and Firefox browser use,
desktop Chromium installed use, Android Chrome browser/installed use, and iOS
Safari browser/Home Screen use. Record installation instructions and unavailable
features per tested cell. [Installability varies by browser](https://developer.mozilla.org/en-US/docs/Web/Progressive_web_apps/Guides/Making_PWAs_installable);
do not require a universal install prompt or promise iOS background execution,
continuous hidden-tab polling or push. Freeze versions before acceptance.

## Boundedness and acceptance

Choose and register numerical client/server budgets before measurement: concurrent
requests, rows, chart points, rendered spans, response bytes, poll frequency,
request deadlines, retry count and retained browser memory. Poll only visible,
authorized screens, back off on pressure and never overlap a previous poll.
Cancel superseded client requests and reject stale responses using query/session
generation IDs. Browser abort does not stop current blocking server query work:
the existing two query permits remain held until completion. Preserve this bound
and register server execution/response limits rather than assume cancellation
frees capacity. Client limits do not establish whole-server memory bounds.

| Gate | Required candidate evidence |
| --- | --- |
| U1: integrated workflows | Exact packaged assets and API: passkey login → overview → each Explore signal → next page → trace/missing parent/related logs → pause/resume tail → pipeline → policy settings → passkey enrollment/recovery/session revocation → Spindle enroll/configure/pause/resume/revoke → logout. Independent producer fixtures grade rows, charts and evidence labels. |
| U2: failure semantics | Empty and incomplete answers, gaps, retention expiry/410, 401, 403, 429, 503, timeout and transport failure; delayed/out-of-order responses and filter/session changes cannot overwrite newer state or leak another account's results. |
| U3: privacy/PWA | Inspect browser caches/storage and worker interception using sensitive fixtures; cold offline start, disconnect, logout, shared-device account switch, revoked session, reopened installed app and multiple tabs reveal no persisted telemetry or credentials. Representative cache/auth defects must be rejected. |
| U4: lifecycle/support | HTTPS trust, manifest/icons/scope, asset MIME/CSP/cache headers, deep links, install/relaunch/uninstall, browser version cells, update/rollback/compatibility and interrupted updates verified on exact artifacts. |
| U5: accessibility | Responsive desktop/mobile, both themes, keyboard/focus, screen-reader status announcements, contrast, zoom/reflow, accessible authentication and touch targets checked against the WCAG 2.2 AA target. |
| U6: resource impact | Register fixture, viewport, network, hardware, timing definitions and limits; measure compressed WASM/assets, cold/warm start, browser CPU/memory, bounded long-tail sessions and server/companion behavior with active console polling during existing release trials. |

The accessibility target follows [WCAG 2.2](https://www.w3.org/TR/WCAG22/), including
minimum target size and accessible authentication; no conformance claim precedes
the corresponding checks. Dashboard numbers and trace visuals need text alternatives.
Server package caps and delivery/query gates remain unchanged with the UI active.
Record all commands, exit statuses, exact artifact identities and limits. Native
and finite browser checks have run; the [verification matrix](../formal/verification-matrix.md#identity-and-operator-console)
distinguishes those results from completion of these release gates.
Builds and verification use the required resource launcher/data
drive; browser/guest descendants need explicit containment in registered harnesses.

## Execution dependency

First register identity/session, wire compatibility, privacy, budgets and U1–U6
fixtures as separate policy work. Then implement authorized APIs, Leptos screens,
the PWA/asset lifecycle and package integration; verify negative controls before
candidate acceptance. Include UI-active load in the release matrix and perform
the fresh-state operator walkthrough before release publication. Missing auth,
unsupported advertised workflows, sensitive cache exposure, misleading complete
answers or absent browser/resource evidence block the release. No prototype or
static mockup satisfies these gates.
