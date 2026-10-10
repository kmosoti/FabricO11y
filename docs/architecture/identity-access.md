# Identity and access

The [access adapter](../../crates/fabric-server/src/access.rs) and
[console HTTP boundary](../../crates/fabric-server/src/console.rs) implement the
local passkey and scoped-access design. Candidate acceptance is registered in the
[console protocol](../experiments/formal/console-access-protocol.md); implementation
alone does not establish release qualification. Optional OIDC federation is later work.
The [operations guide](../access-operations.md) owns setup, credentials and recovery
commands; the [security map](../formal/console-security-map.md) records the selected
ASVS controls and their verification limits.

## Boundary and present evidence

The target is one operator-controlled server with local humans/PWA sessions,
workload API clients (including AI), and remote Spindles.
This establishes scoped access within that installation; it makes no multitenant
isolation, cloud IAM, universal attack resistance or compromised-host claim.
The OS administrator, TLS termination and serving origin remain trusted boundaries.
The [access flow](../diagrams/identity-access.mmd) separates human sessions,
workload credentials, query publication, control intent and offline recovery.

Normal `fabric-server serve` requires configured HTTPS origin and RP ID, serves
the packaged PWA and scoped API, and rejects the shared-master admin routes.
Explicit `serve-legacy` is a migration entrypoint for historical installations;
it cannot satisfy authenticated release acceptance. Node tokens protect
intake/polling and minimal health remains public.
[Control](../../crates/fabric-server/src/control.rs) stores per-node token hashes
and refuses authority after uncertain publication. Its
[view](control-plane.md) explicitly permits previously admitted batches to commit
after revocation. Access publication and control publication use a durable intent:
uncertain completion quarantines access until offline reconciliation. Existing
[CTRL/DEL/HIST evidence](../formal/verification-matrix.md) does not establish the
new access properties; they have separate fixtures and browser acceptance.

Immutable, server-generated principal IDs identify humans, workloads and Spindles;
display names, node labels and credential IDs are separate attributes. IDs are
never reassigned. Renaming or key rotation preserves identity and audit attribution.
Deletion retains bounded identity tombstones; no credential grants authority by name.

HTTP/TLS, WebAuthn verification, randomness, time and persistence belong outside
the pure core. Authorization decisions consume explicit principal, grants,
resource and policy-version inputs, respecting [ADR-0015](../decisions/ADR-0015-adopt-a-hexagonal-architecture.md)
and [ADR-0016](../decisions/ADR-0016-keep-a-pure-semantic-core.md).

## Human authentication and recovery

Use pinned `webauthn-rs 0.5.5` for protocol and signature verification. The
[upstream origin-validation advisory](https://github.com/kanidm/webauthn-rs/security/advisories/GHSA-22w3-693w-x895)
affected earlier versions and is fixed in this release. Upstream describes a
security review, but an independently examined report for this exact version is
not available; no audit claim is made here. Record tested browser/authenticator combinations.
Require user verification, a fresh server-generated one-use challenge tied to
the ceremony and account, the configured exact HTTPS origin and RP ID, and library
validation of signatures and credential ownership. Reject unexpected ceremony
types, expired/replayed challenges, wrong origins/RP hashes and missing verification.
Registration needs authorized enrollment; assertion success never selects a role.
[W3C WebAuthn verification](https://www.w3.org/TR/webauthn-2/#sctn-verifying-authentication-assertion)
is the protocol reference. Proposed challenge expiry: 5 minutes.

Configure a stable HTTPS hostname, certificate and RP ID during installation;
do not derive them from arbitrary Host or forwarding headers. Allow proxy headers
only from explicitly trusted termination. Changing the RP ID is a credential
migration, not a config rename. Support platform/synced and hardware passkeys;
signature-counter behavior follows the reviewed library and device capabilities,
not an assumption that all authenticators maintain increasing counters.

The shared bootstrap, additional-key, invitation and recovery enrollment helper
requests discoverable credentials with `residentKey=required` and the legacy
`requireResidentKey=true`, while preserving required user verification and the
library's opaque registration state. Existing nonresident credentials remain
usable through explicit principal sign-in. The server cannot establish resident
storage from unsigned client extension output; browser acceptance must separately
witness the credential in its independent authenticator fixture.

Bootstrap is a local owner operation: generate a 256-bit one-time setup secret
in a protected file, with 10-minute expiry and no public registration mode. Bind
its use to the configured HTTPS ceremony; persist the first admin and consumed
bootstrap state atomically before granting a session. Concurrent attempts produce
at most one first admin. Never reopen bootstrap automatically after corruption.

Strongly recommend a second independent passkey; hardware keys are optional.
First setup must provide the tested offline owner recovery procedure. Permit lost-key
revocation with fresh passkey verification; revoke affected sessions too.
Another authorized admin may disable an account and issue a short-lived, one-use
reenrollment invitation after an explicit operator identity check. No email,
security-question or permanent bearer-master bypass is part of this profile.
Reject removal of the last usable admin/recovery method. Total admin lockout uses
a documented offline owner recovery procedure with the service stopped, protected
state backup and audited credential replacement; it increments credential epochs,
revokes sessions/delegations and never silently restores revoked credentials.

After login, use opaque random server-side sessions in a `__Host-` cookie with
`Secure`, `HttpOnly`, `SameSite=Strict`, `Path=/` and no Domain. Rotate the session
ID after login and privilege changes. Proposed expiry: 30-minute idle timeout,
8-hour absolute lifetime; fresh passkey verification within 5 minutes for
credential, grant, recovery and administrator changes. Logout, account disable
and key compromise invalidate server state. Protect cookie-authenticated mutations
with an origin check and session-bound CSRF token; GET never changes state.
These server-side session choices follow
[OWASP sessions](https://cheatsheetseries.owasp.org/cheatsheets/Session_Management_Cheat_Sheet.html)
and [CSRF guidance](https://cheatsheetseries.owasp.org/cheatsheets/Cross-Site_Request_Forgery_Prevention_Cheat_Sheet.html).
Bound challenge/session counts and attempts; invalid login responses do not reveal
account existence. Clock rollback invalidates time-sensitive authority until resolved.

The PWA caches only versioned public shell assets. API responses, credentials,
audit records and telemetry must bypass service-worker/offline caches; logout
clears any transient view state. No session/token in localStorage, IndexedDB,
URLs or browser-visible config. Render telemetry as untrusted text, apply a
restrictive CSP and same-origin API policy, and verify stored-XSS rejection.
Browser extensions and a compromised serving origin remain residual risks.

## Workloads, AI and Spindles

Each system or AI client has its own workload principal and explicit grants.
Issue 256-bit opaque random bearer secrets once over TLS; store only their
cryptographic hashes plus principal, credential ID, server audience, action/resource
scope, issued/expiry times and revocation epoch. Secret comparison is constant time.
Never share a master/admin token across clients or ship one in PWA assets.
Proposed workload lifetime: 24 hours, hard maximum 30 days; rotation overlap is
explicitly bounded to 10 minutes. Revoke individual credentials or the principal;
rotation preserves the principal and old audit attribution. Audience binding is
verified against the configured server identity; a bearer is still replayable by
its holder until expiry/revocation and is not sender-constrained cryptography.

Delegated AI authority is the intersection of the delegating human's current
grants, workload grants, requested actions/resources and delegation policy.
Bind the resulting credential to the immutable workload actor, human `on_behalf_of`,
audience and policy versions; expiry is at most 15 minutes and cannot outlive
either parent credential/delegation. Parent disable or grant removal invalidates
the delegation. Audit both IDs. Prompts, retrieved telemetry, model output and
client-provided role/actor fields never grant privileges. No recursive delegation
or AI identity/grant administration in this release. Unattended AI uses explicitly
approved workload grants and has no invented human attribution.

Spindles retain a separate credential class, bound to one enrollment and the
existing Strand identity rules. They may submit batches and poll/report their
own configuration only; telemetry attributes cannot impersonate another node.
Pause still permits delivery of already committed Spool data. Revocation retains
unACKed data and terminal enrollment semantics. This release keeps revocable,
long-lived Spindle credentials; workload TTL does not apply. Routine rotation is
follow-up work requiring an explicit offline-Spool recovery/migration design.
The dedicated server Spindle has its own enrollment and protected credential,
never administrator authority, under the [companion contract](../PRODUCT-CONTRACT.md#linux-installation-contract).
Changing authorization must preserve [delivery](delivery.md) and [control](control-plane.md)
bytes, ACK, deduplication, sync ordering and last-valid configuration behavior.

## Authorization and visibility

Every API request is denied unless the server authorizes its action and resource.
Roles are grant templates; assignment alone does not supply unrestricted node
scope. Proposed action vocabulary and defaults:

| Template/class | Actions within explicitly granted scope | Excluded by default |
| --- | --- | --- |
| Viewer | `telemetry.read`, `inventory.read` | Configuration, enrollment, credentials, grants, audit |
| Operator | Viewer plus `node.configure`, `node.pause`, `node.resume` | Enrollment/revoke, identity/grant changes, audit |
| Admin | Operator plus `node.enroll`, `node.revoke`, `identity.manage`, `grant.manage`, `audit.read` | Global scope unless explicitly assigned; direct intake impersonation |
| Workload | Individually granted read/control actions; no role by inference | Identity/grant/recovery administration |
| Delegated AI | Intersection described above | Authority expansion and recursive delegation |
| Spindle | `batch.submit`, `config.poll`, own enrollment only | Query, inventory and administrative actions |

Read grants identify immutable Spindle enrollment IDs and signal classes
(metrics, logs, traces), with bounded query windows/limits. No arbitrary source,
field, service or individual-trace ACL is promised. Co-mingled unauthorized sources
require a separate Spindle; allowed telemetry can itself contain secrets. Enroll
authority includes an approved enrollment namespace and maximum population;
configure authority includes allowed absolute log paths and interval range.
Local Spindle validation, OS permissions and existing resource ceilings still
apply; an admin grant cannot override them. Grant administration requires
explicit scope and fresh verification; a manager cannot delegate a wider scope
than its own delegable authority. The first owner admin receives explicit
installation-wide grants. The server applies current policy on every request,
as recommended by [OWASP authorization](https://cheatsheetseries.owasp.org/cheatsheets/Authorization_Cheat_Sheet.html).

Apply scope inside query planning and exact row selection, not after response
construction. Aggregate/rate inputs, pruning evidence, bounds, completeness,
freshness, gaps, unavailable sources and retention windows must describe the
authorized view without exposing excluded nodes, source paths or counts.
Unknown coverage within that view remains unknown. If shared storage metadata
cannot support truthful scoped evidence, report scoped uncertainty without
listing excluded identities; never claim completeness by omitting relevant loss.
Trace joins/parent lookups return authorized spans only and disclose no hidden
span attributes or existence. Filter inventory/config errors by authorized
enrollments. Direct IDs, exports and error messages receive the same policy.

Pagination binds the immutable principal, effective scope/policy version, query
and retained snapshot in authenticated opaque cursor state. Reject a stolen,
modified, cross-principal or stale-policy cursor before reading history. A policy
change requires restarting the query; snapshot and retention/Gone semantics
remain governed by [retained history](retained-history.md).

## Revocation, persistence and bounds

Authenticate and authorize headers/routes before expensive body buffering,
decoding, history scans or intake allocation; retain a small independently bounded
unauthenticated parser/rate-limit budget. Recheck body-dependent resource scope
before scheduling work. The console middleware checks credentials and route actions
before body extraction; handlers validate body-dependent scopes, and query answers
recheck current policy and expiry before returning data.
Persist policy, credential epoch and revocation transitions atomically and durably;
uncertain writes, malformed identity state or unsupported versions fail closed.
Do not continue serving with a permissive empty policy or a cached old authority.

Serialize mutation authorization with its durable control publication; policy
version rechecks alone without synchronization do not close the race. Recheck
reads before response release and cancel/discard output if grants changed.
Linearize batch authorization at intake admission: a batch admitted before
revocation may finish its existing custody transition; admission after committed
revocation must fail. Bound queued work by existing limits and report the measured
in-flight interval; do not claim instantaneous erasure or revoke bytes already sent.
If stronger intake cancellation is required, register a custody/protocol change.

Keep access state and audit storage outside telemetry retention. Proposed limits:
16 MiB access state; 1,024 human/workload principals; 8 passkeys/16 credentials per principal; 4,096 sessions/1,024 ceremonies.
Reserve 128 MiB disk for durable access-state publication/backups and 256 MiB for
rotating audit, retaining at most 30 days or the byte cap, whichever occurs first.
These reservations stay within existing service/cgroup and aggregate resource budgets.
Preserve existing node inventory bounds. Reject over-cap transitions before writing.

Audit bounded events for login failures/successes, credential lifecycle, recovery,
grant/control changes, authorization denials and delegation use: event/time,
request ID, actor, optional human delegation, action, stable scoped resource,
policy version and outcome. Limit each event to 4 KiB; throttle repetitive denied
events and expose dropped counts. No bearer, cookie, challenge, private key,
full telemetry body or unsanitized log-path/error payload is logged.
[OWASP logging](https://cheatsheetseries.owasp.org/cheatsheets/Logging_Cheat_Sheet.html)
informs these exclusions. Security/control mutations require durable audit and
state publication as one recoverable transition; if this cannot be completed,
fail closed. A separate minimal recovery reserve must allow offline repair without
turning an audit-full condition into an unaudited remote privilege change.

Backup protected access state, passkey public metadata and audit separately from
telemetry. Reject online restore. Restore requires offline owner action and a fresh installation token
epoch: discard sessions/delegations, invalidate restored workload tokens, reconcile
node revocations against current owner records, and explicitly recover node
authority while preserving Spool custody. Uncertain restore state fails closed;
an old backup must never resurrect authority automatically. Register and test the epoch mechanism.

## Required adversarial evidence and remaining decisions

Register independently specified fixtures and expected denials before acceptance:

| Hypothesis | Discriminating checks and required negative controls |
| --- | --- |
| Passkey ceremonies establish the intended human | Replay, wrong origin/RP ID, wrong credential/account, absent UV, parallel bootstrap; bypass each validation and require checker rejection |
| Humans can recover without a mandatory hardware key | Lost sole key and total admin lockout through offline owner recovery; second-key recovery and lost-key revocation; unauthenticated reenrollment must fail |
| Sessions resist fixation/CSRF and respect lifetime | Prelogin ID reuse, forged origin/token, logout, disable, key revoke, timeout, rollback, stale PWA cache; omit each protection and require rejection |
| Scopes contain every read surface | Disjoint enrollment/signal fixtures across metrics/logs/traces, joins with hidden parents, inventory, errors, bounds, completeness and pages; inject one excluded row/metadata field and require rejection |
| Machine/AI credentials cannot escalate | Wrong audience, expired/rotated/revoked token, node-to-admin crossover, widened delegation, spoofed actor, malicious prompt; widening any intersection must fail |
| Revocation races have a defined bound | Barrier-controlled read/control/intake races and queued work; stale-policy publication/response and post-revoke admission mutations must be rejected |
| Access state remains safe under failure and pressure | Restart, corruption, unsupported version, sync/rename fault, online restore, backup rollback, uncertain restore, cap exhaustion and audit-full; permissive fallback or restored authority must fail |

Use real HTTPS/browser/passkey flows and packaged PWA/server paths as well as
deterministic policy tests. Preserve minimized failures with origin/trace and
contract references. Record exact revision, command, exit status, resource/cleanup
receipts and limits in the release evidence. Generated implementation tests alone
are not an independent oracle. No checks in this table have run for this design.

Before implementation, resolve reviewed WebAuthn library availability, supported
hardware/browser recovery behavior, durable audit/state transaction design,
enrollment/signal-scoped storage evidence and protected restore epoch. Register defaults
and adversarial checks in the release plan/matrix without weakening existing oracles.
Evidence supports a bounded claim on a named revision, never universal absence of vulnerabilities.
