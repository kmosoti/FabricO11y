# Authenticated console acceptance protocol

Revision 2, registered 2026-10-10 before the grant/visibility regression rerun.
Revision 1 preceded integrated browser execution; its observations retain their
original source identities. This revision clarifies client behavior within the
unchanged server and grant budgets. This is
the executable acceptance scope for ACCESS-1–4 and UI-1–2 within the
[operator-console milestone](../../milestones/operator-console.md). Native unit
checks are supporting evidence; they do not establish browser interoperability.
Results and observations belong in the research wiki, with local receipt links.
The [ASVS selection](../../formal/console-security-map.md) pins control identifiers,
mechanisms, applicability and explicit differences without a conformance claim.

## Candidate and containment

Freeze source, Cargo.lock, Rust 1.99.0, WebAuthn 0.5.5, console build receipt,
asset hashes and exact server/Spindle binary hashes before each run. Dependency
policy examines the complete workspace, including the WASM target graph.
Use the production `serve` entrypoint with a supervised dedicated Spindle,
configured HTTPS origin/RP ID and staged, hash-verified packaged console assets.
The explicit `serve-legacy` migration path cannot satisfy this protocol.

All commands run through the resource launcher, on the mounted data drive,
with a 20 GiB/no-swap outer cgroup, a 4,000,000,000-byte server subgroup and
finite 30-minute run deadlines. Chrome/Firefox, drivers and validators remain
inside the outer cgroup. Admit at most one browser workload at once. Record
per-boundary CPU, RSS/cgroup peak, elapsed time, disk bytes and cleanup; failure
fixtures and exact command/exit status survive cleanup. Total data stays below
100 GB. Installed packages separately retain their stricter service limits.

Desktop cells: Chrome for Testing 155.0.8059.39 with matching chromedriver,
and Firefox 157.0 with geckodriver 0.37.1, on the measured Linux host. Record
archive URLs, observed archive hashes and actual browser versions. Use a fresh
profile and a virtual CTAP2 authenticator with resident credentials and user
verification. The browser harness may trust one fixture certificate's exact
SPKI; this is controlled test trust, not evidence of operator OS certificate
installation. Physical platform authenticators, Android and iOS are distinct
cells and must not be inferred from desktop emulation.

## Admission and algorithm budgets

Version 1 client requests carry `X-Fabric-Client-Version: 1`. The API accepts
same-origin opaque human sessions or scoped workload credentials; never both.
Cookie mutations require the exact configured Origin and session CSRF token.
Fresh human verification is required for identity and grant changes.

Public ceremonies: 8 requests/s and 8 live requests. Protected API: 32 credential
checks/s and 16 live requests. Both have a 15-second request deadline and a
64 KiB body limit. Query execution retains its two permits until blocking work
finishes, including after request cancellation. Requests that cannot be admitted
receive an explicit error; no silent queue growth or partial successful answer.

Scoped queries permit logs, metric points and spans, at most 24 hours and
1,000 rows (or a smaller grant), 1 MiB conservatively estimated retained row
payload, 4,096/1 MiB gap evidence and an 8 MiB serialized response. The scoped
source map filters enrollments before selection and derives evidence only from
authorized sources/signals. Shared unreadable storage produces redacted scoped
uncertainty. Unpaginated rate queries are unavailable through this adapter.
An opaque page handle is bound to principal, effective scope, delegation context,
policy, query and snapshot; 1,024 handles expire after 15 minutes.

The client requests at most 200 rows within the current grant, admits one active request, aborts after 15 seconds,
caps response bytes while streaming, and rejects stale session/query generations.
Tail polling displays bounded snapshots, not cross-poll delivery/deduplication:
the current projection lacks generation identity. Its display cap is 200 rows
and 256 KiB; truncation and poll gaps remain visible. Charts preserve exact
integer timestamps/values in text and bound plotted extrema independently.

Grant-limited defaults and presets must expose their effective duration. An
explicit exact window beyond the grant is rejected without silently narrowing
it. A 100-row/60-second grant is the regression fixture: it must permit a valid
60-second query, reject an explicitly requested 15-minute query, and never
convert zero authority into a usable default. Hidden tabs issue no tail polls;
visibility resumes the existing bounded cadence without overlapping requests.

## Registered checks and counterexamples

| Cell | Required observation |
| --- | --- |
| C1 | Protected local bootstrap creates exactly one owner through real browser WebAuthn; concurrent/replayed bootstrap and first-visitor ownership fail. Session cookie is Secure, HttpOnly, SameSite=Strict, host-only and path `/`. |
| C2 | Logout, fresh login, a second passkey, session revocation, human invitation, workload issuance/rotation, narrowed delegation, parent logout/revocation and offline owner recovery behave as documented. Expired or wrong-origin/RP/UV/challenge credentials never authorize. |
| C3 | Two fixed enrollment fixtures have distinct canary rows and freshness values. A log-only grant for A cannot return B's rows, inventory, paths, gaps, retention bounds, global snapshot position, or A's metric timestamp. Querying a denied signal/source fails. A copied, modified, expired or differently scoped cursor fails before history execution. |
| C4 | Real UI sign-in, each signal query, next page, span detail/missing parent, related-log search, tail pause/resume, pipeline, retention policy, identity and Spindle controls, and logout use the production API. Independent producer constants grade returned values, not screenshot appearance. |
| C5 | API/auth responses are no-store and never enter worker caches or persistent browser storage. Disconnect, logout, account change, delayed responses, multiple tabs and reopened offline shell clear sensitive display state. Injecting API caching must fail this check. |
| C6 | Shell assets, MIME, CSP, worker scope and manifest match the build. Update/rollback, open tabs, interrupted update and unsupported client version either retain one coherent shell or require an update; no incompatible write. Desktop installed-PWA checks are distinct from an ordinary browser tab. |
| C7 | Both themes, keyboard/focus, status announcements, readable tables, zoom/reflow and selected text contrast are exercised. Automated checks do not establish untested assistive-technology or physical-device conformance. |
| C8 | Record cold/warm asset/start timing, CPU/memory and bounded sustained polling alongside server/Spindle accounting. A data read timeout leaves no extra scan admission slot. Payload/gap pressure produces explicit rejection and preserves existing custody/query invariants. |

Negative controls include a cache-all API worker, a scoped-metadata leak,
authentication after body extraction, cross-scope page reuse and recovery cuts
at each durable publication boundary. Each must fail its named property, rather
than an unrelated startup error. Existing independent delivery/query/rate oracles
and historical protocols remain unchanged.

Record every cell as passed, failed, interrupted or unrun against its exact
candidate. A virtual authenticator is an independent browser/protocol fixture,
not a claim of physical authenticator, mobile installation or universal security
qualification. A failed or unavailable required cell blocks the corresponding
release claim until resolved or explicitly rescoped by the owner.
