# Identity and access

The [access adapter](../../crates/fabric-server/src/access.rs) and
[console HTTP boundary](../../crates/fabric-server/src/console.rs) implement the
local passkey and three-role design of [ADR-0029](../decisions/ADR-0029-use-three-fixed-roles-for-the-first-release.md).
Acceptance checks are registered in the
[console protocol](../experiments/formal/console-access-protocol.md); implementation
alone does not establish release qualification. Optional OIDC federation is later work.
The [operations guide](../access-operations.md) owns setup, credentials and recovery
commands; the [security map](../formal/console-security-map.md) records the selected
ASVS controls and their verification limits, and [security hardening](security-hardening.md)
the admission, lock, credential-file and sandbox controls.

## Boundary and present evidence

The target is one operator-controlled server with one human owner (a browser PWA),
read-only script and AI clients, and remote Spindles. This establishes role
separation within that installation; it makes no multitenant isolation, cloud IAM,
per-source confinement, universal attack resistance or compromised-host claim.
The OS administrator, TLS termination and serving origin remain trusted boundaries.
The [access flow](../diagrams/identity-access.mmd) shows the three roles.

Normal `fabric-server serve` requires a configured HTTPS origin and RP ID (it exits
without them: `serve_refuses_to_start_without_access_origin`), needs no administrator
token, serves the packaged PWA and console API, and rejects the shared-master admin
routes. Explicit `serve-legacy` is a migration entrypoint, the only mode that reads
`admin_token_file`, and cannot satisfy authenticated release acceptance. Minimal health remains public. [Control](../../crates/fabric-server/src/control.rs)
stores per-node token hashes and refuses authority after uncertain publication; its
[view](control-plane.md) permits previously admitted batches to commit after revocation.
Access and control publication share a durable intent: uncertain completion quarantines
access until offline reconciliation. HTTP/TLS, WebAuthn verification, randomness, time
and persistence stay outside the pure core ([ADR-0015](../decisions/ADR-0015-adopt-a-hexagonal-architecture.md),
[ADR-0016](../decisions/ADR-0016-keep-a-pure-semantic-core.md)).

## Roles and routes

| Role | Credential | Reaches |
| --- | --- | --- |
| Owner | Passkey with user verification, then a `__Host-` session cookie; mutations also need the CSRF token and exact Origin | Every console route |
| Reader | Expiring (at most 30 days), individually revocable 256-bit bearer token; only its SHA-256 is stored | `POST /v1/console/query`, `GET /v1/console/status`, `GET /v1/console/nodes` |
| Spindle | Per-node token over verified TLS | `POST /v1/batches`, `GET /v1/config`: own intake and configuration only |
| Anonymous | none | `GET /v1/health`, the public shell, and the passkey ceremony routes under their own admission limits |

One function maps method and path to the minimum role and defaults to owner, so a new
route is owner-only until listed. The role comes from the credential type alone. A
Spindle token is not a console credential, a console credential is not a Spindle
token, and `/v1/admin/*` is refused for everyone on the console router. Owner-only
routes: `session`, `logout`, node enrollment, configuration and control, `readers`,
`audit`, `passkeys` and `sessions`. "Status" means the server status and the Spindle
inventory; readers never see credentials, passkeys, sessions or audit. Every
state-changing method re-checks the owner role in the access store, independent of
the route guard. Reader activity is not audited; denials, credential lifecycle,
recovery and control are.

## Owner authentication and recovery

Protocol and signature verification use pinned `webauthn-rs 0.5.5`. The
[upstream origin-validation advisory](https://github.com/kanidm/webauthn-rs/security/advisories/GHSA-22w3-693w-x895)
affected earlier versions and is fixed in this release. Upstream describes a security
review, but no independently examined report for this exact version is available; no
audit claim is made. Ceremonies require user verification, a fresh server-generated
one-use challenge tied to the ceremony and account (5-minute expiry), the configured
exact HTTPS origin and RP ID, and library validation of signatures and credential
ownership. Unexpected ceremony types, expired or replayed challenges, wrong origins
or RP hashes and missing verification are rejected
(`signed_assertions_with_wrong_origin_rp_id_or_missing_uv_are_rejected`).
[W3C WebAuthn verification](https://www.w3.org/TR/webauthn-2/#sctn-verifying-authentication-assertion)
is the protocol reference. Registration needs the bootstrap secret or an owner session;
assertion success never selects a role.

Configure a stable HTTPS hostname, certificate and RP ID at installation; never derive
them from Host or forwarding headers. Changing the RP ID is a credential migration.
Platform, synced and hardware passkeys are supported; signature-counter behavior
follows the library and device. Bootstrap, additional-key and recovery enrollment
request `residentKey=required` and `requireResidentKey=true` with required user
verification and opaque library state. The server cannot establish resident storage
from unsigned client output; browser acceptance witnesses it independently.

Bootstrap is a local owner operation: a 256-bit one-time secret in a protected file,
10-minute expiry, no public registration mode, consumed atomically with the first owner.
Concurrent attempts yield at most one owner. Bootstrap never reopens automatically.
A second independent passkey is strongly recommended; the last passkey cannot be
revoked. Lost-key recovery with the service stopped uses the local root CLI: it
rotates the installation epoch, removes the owner's passkeys, invalidates sessions and
reader tokens, and issues a fresh 10-minute secret. No email, security-question or
permanent bearer-master bypass exists.

Sessions are opaque random server-side values in a `__Host-` cookie (`Secure`,
`HttpOnly`, `SameSite=Strict`, `Path=/`, no Domain), rotated at login. Limits: 30-minute
idle, 8-hour absolute (`sessions_expire_at_the_idle_and_absolute_limits`); a passkey check within 5 minutes before passkey, session and
reader-token changes. Logout, passkey change and recovery invalidate server state.
Cookie mutations need the exact Origin and a session-bound CSRF token; GET never changes
state. Bound challenge and session counts; unknown principals receive the same
challenge shape. Clock rollback poisons time-sensitive authority until resolved. These
choices follow [OWASP sessions](https://cheatsheetseries.owasp.org/cheatsheets/Session_Management_Cheat_Sheet.html)
and [CSRF guidance](https://cheatsheetseries.owasp.org/cheatsheets/Cross-Site_Request_Forgery_Prevention_Cheat_Sheet.html).

The PWA caches only versioned public shell assets. API responses, credentials, audit
records and telemetry bypass service-worker and offline caches, and logout clears
transient view state. No session or token is stored in localStorage, IndexedDB, URLs or
browser-visible config. Telemetry renders as untrusted text under a restrictive CSP.
Browser extensions and a compromised serving origin remain residual risks.

## Readers and Spindles

The owner issues a reader token once over TLS, with a name and lifetime; it is never
shown again. Each is stored as hash, name, issue and expiry times and the server
audience (origin plus installation epoch); comparison is constant time. At most 256
tokens exist; expired ones are pruned at issuance. Rotation is issue-then-revoke, with
no overlap rule. Revocation and expiry apply to every later check, including a read
in flight before its answer is released. A bearer is replayable by its holder until
expiry or revocation. Readers see all telemetry; narrower roles need a later decision.

Spindles keep a separate credential bound to one enrollment and the existing Strand
identity rules: they submit batches and poll or report their own configuration only,
never query or administer. Pause still permits delivery of committed Spool data;
revocation retains unACKed data. Spindle credentials are long-lived and revocable;
rotation needs a separate offline-Spool design. The dedicated server Spindle has its
own enrollment and protected credential, never administrator authority, under the
[companion contract](../PRODUCT-CONTRACT.md#linux-installation-contract). Authorization
changes preserve [delivery](delivery.md) and [control](control-plane.md) bytes, ACK,
deduplication, sync ordering and last-valid configuration behavior.

## Query bounds, pagination and visibility

Owner and reader share one query bound: at most 1,000 rows, a 24-hour half-open window
and an 8 MiB response; rate queries are unavailable. Inventory, aggregates, completeness,
freshness, gaps and errors describe all telemetry, and unknown coverage stays unknown.
The query layer still receives the enrolled-source set it filtered by before ADR-0029;
for both roles that set is the complete inventory, so it excludes nothing.
A page handle binds the immutable credential principal, the query and the retained
snapshot; another credential, a modified query or an expired handle is rejected before
history is read. Snapshot and retention/Gone semantics remain governed by
[retained history](retained-history.md).

## Revocation, persistence and bounds

The console authenticates and chooses the role from headers and route before body
buffering, decoding or history scans, within a small independently bounded
unauthenticated budget; handlers re-validate CSRF, freshness and current authority, and
a read's answer re-checks its credential before release. State is written with `write`,
`rename` and directory sync. Uncertain writes, malformed state or an unsupported format
fail closed; format 2 replaces format 1, and older state is refused with a re-bootstrap
error rather than migrated.

Mutation authorization is serialized with its durable control publication. A batch
admitted before committed revocation may finish its custody transition; admission after
it fails. Access state and audit live outside telemetry retention. Limits: 16 MiB state;
one owner with 8 passkeys; 256 reader tokens; 4,096 sessions; 1,024 ceremonies; audit
events kept 30 days, at most 4,096 outside denials plus 256 denials, which are capped
apart and never evict another event; 128 MiB reserved for state publication and backups
and 256 MiB for rotating audit. Over-cap transitions are rejected before writing.

Audit records login results, credential lifecycle, recovery, control and denials: time,
request ID, actor, action, stable resource and outcome, at most 4 KiB each. At most one
denial is stored per second; the rest, and any event evicted by a cap, add to the
`suppressed_or_expired_events` count (age expiry is not counted despite its name). No bearer, cookie, challenge, private
key, telemetry body or unsanitized path is logged ([OWASP logging](https://cheatsheetseries.owasp.org/cheatsheets/Logging_Cheat_Sheet.html)).
Control and security mutations need durable audit and state publication as one
recoverable transition, or they fail closed. Back up access state and audit separately
from telemetry. Online restore is refused; offline restore needs a fresh installation
epoch, which discards sessions and reader tokens, so an old backup never resurrects authority.

## Evidence

The [console protocol](../experiments/formal/console-access-protocol.md) registers the
checks and negative controls: passkey, session and CSRF rejections, the role matrix over
every console and Spindle route (widening any role must fail), reader expiry, revocation
and cross-credential cursor reuse, revocation races, and restart, corruption,
older-format, fault, restore, cap and audit-full cases. Generated implementation tests
are not an independent oracle. Evidence supports a bounded claim on a named revision,
never universal absence of vulnerabilities.
