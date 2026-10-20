# ADR-0029: Use three fixed roles for the first release

## Status

Accepted on 2026-10-11 by the owner (issue #49). Partially supersedes
[ADR-0027](ADR-0027-ship-a-scoped-passkey-pwa-console.md): its scoped human,
workload and delegated access is replaced by the roles below. Its passkey
sign-in, bounded sessions, shell-only PWA, offline owner recovery and separate
Spindle credential stand. The implementation is present in the current source;
candidate/package acceptance remains pending.

## Context

ADR-0027 gave every principal a scope: actions, Spindle enrollments, signal
classes, query windows and row caps, log paths, intervals and enrollment
namespaces. It added further humans by invitation and delegation of narrowed
authority to AI. Each dimension needs enforcement in query planning, inventory,
pagination, errors and the UI, plus cross-principal fixtures that prove nothing
leaks. The first release has one operator, scripts and agents that read, and
Spindles. None of its workflows needs a narrower grant than "read everything".
At the base of this change `access.rs` holds 3,074 lines, `console.rs` 1,226 and
the console's `live.rs` 1,629, much of it serving those dimensions.

## Decision

The alpha has three fixed roles and no other principal type.

| Role | Credential | Routes |
| --- | --- | --- |
| Owner | One human principal with one to eight passkeys (user verification required) opens a `__Host-` session cookie (`Secure`, `HttpOnly`, `SameSite=Strict`, no Domain); mutations need the session CSRF token and the exact configured Origin | All console routes |
| Reader | An expiring (at most 30 days), individually revocable 256-bit bearer token for scripts and AI agents; only its SHA-256 is stored | `POST /v1/console/query`, `GET /v1/console/status`, `GET /v1/console/nodes` |
| Spindle | A per-node token over verified TLS, unchanged ([ADR-0014](ADR-0014-manage-nodes-through-server-control-state.md)) | `POST /v1/batches`, `GET /v1/config` |

The console's route guard maps method and path to a minimum role in one place and
defaults to owner, so a new route is owner-only until listed. Owner satisfies a
reader requirement; reader does not satisfy an owner requirement; a Spindle token
is not a console credential and a console credential is not a Spindle token.
Anonymous callers reach only the health route, the public shell assets and the
passkey ceremony routes, which have their own admission limits. The role comes
from the credential type alone. Names, headers, bodies, prompts and retrieved
telemetry never select it. "Status" means the server status and the Spindle
inventory; readers never see credentials, passkeys, sessions or audit.

Owner and reader read all telemetry through the same bounded query: at most
1,000 rows, a 24-hour window and an 8 MiB response. A page handle is bound to the
credential that created it, and its query shape; another credential receives
denial before any history is read. The query layer's row filter stays as it is.
The console passes it the complete enrolled inventory, so no source is excluded.

Only the owner issues, lists and revokes reader tokens, adds or revokes
passkeys, lists and revokes sessions, reads audit, and enrolls, configures,
pauses, resumes or revokes Spindles. Those credential changes need a passkey
check within five minutes. Rotation is issue-then-revoke; there is no overlap
window and no stored principal for a reader.

Kept unchanged: the one-time bootstrap secret; a second passkey; offline owner
recovery by the local root CLI; the 30-minute idle and 8-hour absolute session
limits; durable audit with throttled denials; per-peer and global admission
limits; refusal of `/v1/admin/*` on the console router; the explicit
`serve-legacy` migration from a master token to a bootstrapped owner.

Removed: additional humans and invitations, per-principal scope and
`set_scope`, delegation, enrollment namespaces, workload principals and their
rotation overlap, principal disable, the policy-version counter that tracked
grant and credential changes, and the routes and UI screens that served them.
Invalidation is now exact: a passkey change bumps the owner epoch and ends the
owner's sessions, and revoking or expiring a reader token ends that token's
access before any in-flight read is released.

Access state format 2 replaces format 1. No release ever shipped format 1, so
the server does not migrate it. Opening older state fails with a message to
re-bootstrap: stop the service, remove `state_dir/access` and start again.

## Alternatives considered

- Keep scoped grants and cut only delegation. Per-Spindle and per-signal scope
  remain the largest source of leak surfaces and fixtures, with no first-release
  workflow that needs them.
- Allow readers to mutate with a bearer token. Scripted control removes the
  passkey and CSRF barrier from every mutation and widens a stolen-token impact.
- Client-certificate readers. They add a private CA and certificate lifecycle for
  a small deployment.
- One shared bearer for owner and readers. This is the ADR-0014 administrator
  credential ADR-0027 already rejected: no read-only separation or revocation.
- Migrate format 1 state. It would carry code for a state no release produced.

## Evidence

Nothing here is measured. The base sizes above are line counts of the source at
the base revision. The [console protocol](../experiments/formal/console-access-protocol.md),
[security map](../formal/console-security-map.md) and release gates B11-B13 are
revised in the same trust-boundary commit as this record. The implementation
reports its own before and after sizes.

## Consequences

Authorization becomes a short, auditable table that a native test can enumerate
for every route and role. Per-source confinement is gone: a leaked reader token
discloses all telemetry until it expires or is revoked, and every reader is
equal. Control is possible only through the owner's browser session, so
`fabricctl access` keeps `node list` and `query`. A second human or a narrower
agent needs a later ADR. Reader queries are not audited, and audit attributes
control to the owner. Development trials must re-bootstrap. The query layer's
now-redundant scoped filtering remains until a later change removes it.

## Validation

A table-driven native test calls every console route as anonymous, Spindle
token, reader token and owner session and requires exactly the matrix above;
widening any role must fail it. All passkey, session fixation, CSRF, origin,
replay, stored-XSS and stale-cursor checks of the
[protocol](../experiments/formal/console-access-protocol.md) remain. Browser
checks add reader expiry, revocation, cross-credential page reuse and a
Spindle token refused by the console. A test opens format-1 state and requires
the re-bootstrap error.

## Related

- [Product contract](../PRODUCT-CONTRACT.md#control-and-security-contract)
- [Identity and access](../architecture/identity-access.md)
- [Release plan](../milestones/release-readiness.md)
- [Security map](../formal/console-security-map.md)
- [Verification matrix](../formal/verification-matrix.md#identity-and-operator-console)
