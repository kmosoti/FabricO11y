# Authenticated console acceptance protocol

Revision 6, registered 2026-10-11 before role-model acceptance. It replaces
scoped principals with the three fixed roles of
[ADR-0029](../../decisions/ADR-0029-use-three-fixed-roles-for-the-first-release.md);
earlier observations are not role-model results.
Revision 5 preceded peer-admission acceptance.
Revision 4 preceded resident-enrollment acceptance.
Revision 3 preceded standalone Firefox CTAP 2.1 acceptance.
Revision 2 preceded the grant/visibility regression rerun; its observations
retain their original source and fixture identities.
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

For the prospective Firefox desktop cell, use the virtual device protocol
`ctap2_1`, USB transport capability, `hasResidentKey=true`,
`hasUserVerification=true`, `isUserVerified=true` and `isUserConsenting=true`.
Select Firefox's software test manager with
`security.webauth.webauthn_enable_softtoken=true` and
`security.webauth.webauthn_enable_usbtoken=false`, and record actual preference
readback. Keep required server user verification and every C1–C8 assertion.
This selection applies to the fixture; it changes no production server policy.

The pinned Firefox 157
[test token](https://github.com/mozilla-firefox/firefox/blob/FIREFOX_157_0_RELEASE/dom/webauthn/authrs_bridge/src/test_token.rs#L418)
deliberately omits the credential ID for a CTAP 2.0 response when a filtered
allowlist contains one key. Its
[bridge](https://github.com/mozilla-firefox/firefox/blob/FIREFOX_157_0_RELEASE/dom/webauthn/authrs_bridge/src/lib.rs#L1139)
restores the ID only when the original allowlist contains one key. The preserved
two-allowed-keys/one-active-virtual-token CTAP 2.0 failure therefore remains
failed; it is excluded from the prospective CTAP 2.1 fixture cell. It does not
establish a Fabric or physical-authenticator defect. The USB manager also takes
precedence over the software manager in the pinned
[dispatch](https://github.com/mozilla-firefox/firefox/blob/FIREFOX_157_0_RELEASE/dom/webauthn/authrs_bridge/src/lib.rs#L939).

Resident-key capability does not establish an actual discoverable credential.
The observed production enrollment requested `residentKey=discouraged`, and
the Firefox credential witness was nonresident. Actual resident-credential
coverage from the original matrix remains unestablished by these diagnostics.
Standalone acceptance must retain its own exact candidate and fixture identity;
concurrent causal diagnostics do not fill that cell.

For the resident-enrollment successor, retain the library's required user
verification and require `residentKey=required` with `requireResidentKey=true`
in public registration options. Observe actual stored credentials independently
through the browser's virtual-authenticator inventory after owner and additional
key enrollment. Every stored credential on the active fixture device must be
resident and belong to the fixture RP. Archive only counts, resident flags and
RP IDs, never credential private keys. Client `credProps.rk` is unsigned and
cannot establish this property as a server security assertion.

The representative negative control changes only the options passed to the
browser to `residentKey=discouraged` and `requireResidentKey=false`; the original
server response, UV requirement and real credential creation remain observable.
It must enroll a real nonresident credential and fail the named resident-inventory
assertion. A startup, transport or enrollment failure does not satisfy that control.
This tests detection of the original default-options defect. It does not add
username-less login or claim that the server can attest credential residency.

## Admission and algorithm budgets

Version 1 client requests carry `X-Fabric-Client-Version: 1`. The API accepts
same-origin opaque owner sessions or reader bearer tokens; never both.
Cookie mutations require the exact configured Origin and session CSRF token.
Fresh owner verification is required for passkey, session and reader-token changes.

Public ceremonies: 8 requests/s and 8 live requests. Protected API: 32 credential
checks/s and 16 live requests. Both have a 15-second request deadline and a
64 KiB body limit. Query execution retains its two permits until blocking work
finishes, including after request cancellation. Requests that cannot be admitted
receive an explicit error; no silent queue growth or partial successful answer.

The production transport wrapper adds per-peer budgets: public ceremonies
2 requests/s, burst 4, two live requests; protected API 8 requests/s, burst 8,
four live requests. Global live ceilings remain eight and sixteen. UI actions
and test probes on localhost share this peer; the older fixture's 10 probes/s
exceeds the protected budget before UI work is counted.

For prospective benign browser acceptance, space explicit API probes and UI
actions by at least 550 ms without retrying responses or replaying writes.
The application and its normal polling remain unchanged. Record request paths,
statuses and Retry-After headers without payloads or credentials. Separately
send sixteen concurrent protected session reads without this fixture pacing;
require an actual HTTP 429, Retry-After and no-store, followed by a successful
read after refill. Preserve every C1–C8 assertion and historical failure.

Console queries permit logs, metric points and spans for owner and reader alike:
at most 24 hours and 1,000 rows, 1 MiB conservatively estimated retained row
payload, 4,096/1 MiB gap evidence and an 8 MiB serialized response. Unpaginated
rate queries are unavailable through this adapter. An opaque page handle is bound
to its credential's principal ID, the query and the snapshot; 1,024 handles
expire after 15 minutes.

The client requests at most 200 rows, admits one active request, aborts after 15 seconds,
caps response bytes while streaming, and rejects stale session/query generations.
Tail polling displays bounded snapshots, not cross-poll delivery/deduplication:
the current projection lacks generation identity. Its display cap is 200 rows
and 256 KiB; truncation and poll gaps remain visible. Charts preserve exact
integer timestamps/values in text and bound plotted extrema independently.

An explicit window beyond 24 hours is rejected without silently narrowing it. Hidden tabs issue no tail polls;
visibility resumes the existing bounded cadence without overlapping requests.

## Registered checks and counterexamples

| Cell | Required observation |
| --- | --- |
| C1 | Protected local bootstrap creates exactly one owner through real browser WebAuthn; concurrent/replayed bootstrap and first-visitor ownership fail. Session cookie is Secure, HttpOnly, SameSite=Strict, host-only and path `/`. |
| C2 | Logout, fresh login, a second passkey, session revocation, reader-token issuance, expiry and revocation, and offline owner recovery behave as documented; recovery invalidates reader tokens. Expired or wrong-origin/RP/UV/challenge credentials never authorize. Access state of an older format is refused with the re-bootstrap error. |
| C3 | Every console route and both Spindle intake routes are called anonymously, with a Spindle token, with a reader token and with an owner session; exactly the ADR-0029 matrix is admitted (a reader reaches only query, status and nodes). Two fixed enrollment fixtures with distinct canary rows and freshness values are both returned, with their own freshness, to reader and owner. A page handle copied between owner and reader, modified or expired fails before history execution. A revoked or expired reader token is refused at once, including for an in-flight read. |
| C4 | Real UI sign-in, each signal query, next page, span detail/missing parent, related-log search, tail pause/resume, pipeline, retention policy, passkey, reader-token and Spindle controls, and logout use the production API. Independent producer constants grade returned values, not screenshot appearance. |
| C5 | API/auth responses are no-store and never enter worker caches or persistent browser storage. Disconnect, logout, account change, delayed responses, multiple tabs and reopened offline shell clear sensitive display state. Injecting API caching must fail this check. |
| C6 | Shell assets, MIME, CSP, worker scope and manifest match the build. Update/rollback, open tabs, interrupted update and unsupported client version either retain one coherent shell or require an update; no incompatible write. Desktop installed-PWA checks are distinct from an ordinary browser tab. |
| C7 | Both themes, keyboard/focus, status announcements, readable tables, zoom/reflow and selected text contrast are exercised. Automated checks do not establish untested assistive-technology or physical-device conformance. |
| C8 | Record cold/warm asset/start timing, CPU/memory and bounded sustained polling alongside server/Spindle accounting. A data read timeout leaves no extra scan admission slot. Payload/gap pressure produces explicit rejection and preserves existing custody/query invariants. |

Negative controls include a cache-all API worker, a reader admitted to an owner
route, a Spindle token admitted by the console, authentication after body
extraction, cross-credential page reuse and recovery cuts
at each durable publication boundary. Each must fail its named property, rather
than an unrelated startup error. Existing independent delivery/query/rate oracles
and historical protocols remain unchanged.

Record every cell as passed, failed, interrupted or unrun against its exact
candidate. A virtual authenticator is an independent browser/protocol fixture,
not a claim of physical authenticator, mobile installation or universal security
qualification. A failed or unavailable required cell blocks the corresponding
release claim until resolved or explicitly rescoped by the owner.

## Prospective signed browser security supplement

Before replacement-candidate execution, a separate `--security-probes` cell uses
only the exact packaged server and freshly enrolled virtual browser credential.
It shares the existing 240-second deadline, resource bounds, shared request
cadence and cleanup. It cannot run together with another lifecycle mode.

Capture the completed registration request only in private browser memory, replay
it once and require 403 `access denied`. Obtain a real browser login assertion,
submit it successfully, then replay the identical completed ceremony and require
the same refusal. No secret, challenge, cookie, CSRF value or credential private
key is written into public receipts.

Export only that fresh virtual fixture credential's private key into protected
scratch. For each fresh challenge, sign a one-field negative: foreign client
origin, foreign RP hash, or absent UV bit. Preserve the other authenticator flags,
credential ID, client-data fields and increasing signature counter. OpenSSL must
verify every signature independently before dispatch. Each negative must receive
403 `access denied`; an adjacent correctly signed assertion must receive 200.
A signature error, 429, missing fixture key or transport failure is inconclusive,
not the named rejection. Never mutate the product access store or production key.

Before a valid login, set an independently chosen attacker cookie in the private
browser profile. The resulting session cookie must differ; the chosen cookie
must receive 401 `sign in required`. Separately submit a successful login carrying
the actual previous cookie and require that previous cookie to receive the same
401 while the replacement works. Record only public hashes and outcomes.

Issue an independent three-second reader and a 60-second reader through the
actual owner API. Require the short reader to succeed before expiry, wait at
least four real monotonic seconds from issuance and beyond its returned Unix
expiry, then require 401 `sign in required`, with adjacent long-reader and owner
200 witnesses. Retain wall/monotonic brackets; a clock discontinuity fails this
ordinary-time cell. This is new-request expiry, not the native in-flight race.

The literal result checker requires each named property exactly once. Source
controls must reject missing/duplicate properties, forged success, wrong error,
429, missing independent signature evidence, and insufficient real wait. A
separate diagnostic `--inject-security-defect PROPERTY` changes only that named
observed result after its real positive mechanism has run; the checker must
reject the named property, never an unrelated startup error. This supplements
existing CSRF, XSS, resident inventory and complete route-matrix cells; it does
not establish their missing coverage or physical-device certification.
