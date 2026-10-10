# ADR-0027: Ship a scoped passkey PWA console

## Status

Accepted intended design on 2026-10-10 following the owner's release scope and
local-passkey decision. Implementation and acceptance remain unrun. This amends
the shared administrative credential boundary of ADR-0014 for the new release;
its existing control transitions and Spindle custody contract remain intact.

## Context

A usable central/edge deployment needs browser investigation and distinct access
for humans, automation and AI. The current HTTP adapter uses one admin bearer
for both queries and control. Reusing that credential in a browser or across
agents would defeat read-only access and individual revocation. A PWA adds
persistent assets, multiple tabs and upgrade behavior to the security boundary.

## Decision

Serve a Leptos client-rendered WASM console and its complete static asset set from
the existing server's HTTPS origin. Include it in both Linux packages. Cache
only public versioned shell assets; telemetry and credentials remain outside
offline application storage. Disconnected startup cannot display protected data.

Use local WebAuthn passkeys through a maintained, reviewed verification library,
protected owner bootstrap/recovery and bounded opaque server-side sessions.
OIDC/SSO is deferred. Human, workload and Spindle principals are distinct.
Expiring, individually revocable workload credentials carry explicit permissions;
delegated AI grants cannot exceed their current parent grants. Spindles retain
their existing separate, revocable intake/configuration credential class.

Enforce deny-by-default action and Spindle/signal scope inside server query and
control boundaries, including metadata, pages and audit attribution. Preserve
unknown evidence within the authorized view. Browser controls do not enforce
security. Crypto, time, persistence and HTTP stay outside the pure core; pure
policy decisions receive explicit inputs. No new core dependency is authorized
by this decision. New API DTOs and crate boundaries require layer registration.

The [identity view](../architecture/identity-access.md) owns the access design;
the [console plan](../milestones/operator-console.md) owns workflows, cache and
browser behavior. Both are mandatory dependencies of release gates B11–B13.
Delivery bytes, sync ordering and previously admitted Batch custody are unchanged.

## Alternatives considered

- External-only OIDC would delegate human identity operation, but require another
  service for small deployments; the owner selected independent local passkeys.
- A shared browser/admin bearer would reuse existing code but provide no narrow
  human sessions, per-client revocation or delegated workload boundary.
- SSR or a separate UI service would add a runtime without an identified need
  for this authenticated console. Static CSR fits the existing server boundary.
- Persisting query results for offline investigations would improve offline
  utility but create a retained-data and revocation boundary absent from scope.

## Evidence

[Current HTTP routes](../../crates/fabric-server/src/http.rs) and
[query types](../../crates/fabric-server/src/query.rs) establish existing shared
admin authentication, query limits and admission behavior. They do not implement
the new design. [Leptos CSR deployment](https://book.leptos.dev/deployment/csr.html),
[WebAuthn](https://www.w3.org/TR/webauthn-2/) and the primary references in the
identity/console views support mechanism selection, not Fabric security results.
The owner's four visual references establish light/dark desktop/mobile direction;
their sample metrics, alert badges and pipeline stages are not product evidence.

## Consequences

Release now depends on identity persistence, protected recovery, scoped query
evidence, browser assets and browser acceptance in addition to storage/deployment.
Stable HTTPS origin/RP configuration becomes an installation requirement.
Existing master-token deployments need an explicit local migration and CLI
credential path; legacy authority cannot remain an undocumented bypass. Resource
limits and delivery/query gates must still hold with active console traffic.

## Validation

Register expected-denial fixtures before implementation acceptance. Exercise real
packaged HTTPS/passkey/PWA workflows, cross-principal reads and mutations,
delegation, replay, CSRF, stored XSS, stale cursors, revocation races, offline and
update behavior, restart/restore and bounded audit pressure. Each checker must
reject a representative injected defect. Pin applicable OWASP ASVS requirements
in the release protocol; neither document review nor this ADR establishes a pass.

## Related

- [Product contract](../PRODUCT-CONTRACT.md#control-and-security-contract)
- [Release plan](../milestones/release-readiness.md)
- [Control plane](../architecture/control-plane.md)
- [Verification matrix](../formal/verification-matrix.md#identity-and-operator-console)
