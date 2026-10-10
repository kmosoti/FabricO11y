# Console security verification map

This candidate uses [OWASP ASVS 5.0.0](https://github.com/OWASP/ASVS/tree/v5.0.0)
as a control reference. It does not claim an ASVS level or independent security
certification. The pinned upstream JSON has SHA-256
`bcdbec214d70abcfad9284a31d4f9e5134305831d628aad3aa85d7e26626cb35`.
Identifiers below refer to that release, not the mutable main branch.

The [identity view](../architecture/identity-access.md) defines the threat boundary;
the [console protocol](../experiments/formal/console-access-protocol.md) defines
finite candidate checks. A source reference identifies the mechanism to inspect,
not a passing result. Browser results must name exact binaries, assets and tests.

| ASVS 5.0.0 controls | Fabric boundary and required evidence |
| --- | --- |
| V1.2.1–3, V3.2.1–3 | Leptos renders telemetry as text. JSON serializers own escaping; no untrusted HTML templates. Browser stored-XSS fixture and MIME checks. |
| V1.2.4–5 | Typed query enums, literal substring filters and argument-vector process launches. No SQL or shell interpreter receives query text. Input/path rejection fixtures. |
| V1.4.1–3, V1.5.2 | Rust ownership, bounded response/selection/body handling, typed deserialization. Native limits, cancellation and unchanged query-oracle checks; Rust alone is not memory-bound evidence. |
| V2.1.1–3, V2.2.1–2 | Scope, configuration, query and admission budgets in the protocol; checked again at the server boundary. Oversized/malformed and denied-body fixtures. |
| V3.1.1, V3.7.1, V3.7.5 | Supported browsers require HTTPS, WebAuthn, WASM, CSP and secure cookies. Unsupported authentication stays locked. Versioned browser cells. |
| V3.3.1–5 | One host-only Secure/HttpOnly/Strict opaque session cookie; secret omitted from JSON. Browser cookie inspection and source checks. |
| V3.4.1–6, V3.4.8, V3.5.8 | One-year HSTS, no cross-origin API grant, shell-specific CSP and deny-all non-shell CSP, nosniff, no-referrer, anti-framing, same-origin opener/resource policies. Exact HTTP response checks. HSTS does not claim operator-owned subdomains. |
| V3.5.1–3, V3.5.6–7 | Exact Origin plus session CSRF on mutations; explicit client-version header; no JSONP or private script responses. Wrong-origin/CSRF/method fixtures. |
| V3.5.5 | Cross-tab messages carry lock/update signals, never authority or data; receiving them cannot unlock a session. Multiple-tab browser checks. |
| V6.1.1, V6.1.3, V6.3.1–4 | Explicit passkey, workload and Spindle pathways; bounded ceremony/credential verification; no default account. Production master-token denial and rate/admission fixtures. |
| V6.3.6, V6.4.1–4, V6.4.6 | No email, password hints or question-based recovery. Protected one-use local bootstrap; invited users choose their own passkey; offline owner recovery requires host custody and a stopped server. Browser bootstrap and durable-cut recovery fixtures. |
| V6.3.8 | Missing-account challenges use the maintained library's fake credential generator; opaque IDs and generic failure responses. Compare known/unknown challenge shapes; timing indistinguishability requires separate measurement. |
| V6.5.1–6, V6.7.1–2 | One-use expiring challenges/invitations, 256-bit random setup/credential material, protected state, revocable passkeys and library-verified assertions. Replay, expiry, RP/origin/UV and file-mode fixtures. |
| V7.1.1–2, V7.2.1–4 | Server-owned opaque sessions, 30-minute idle/8-hour absolute lifetime, global 4,096-session bound with explicit refusal. Successful same-browser reauthentication retires the prior cookie session; other devices remain separate. Native and browser session fixtures. |
| V7.3.1–2, V7.4.1–5, V7.5.1–3 | Logout/expiry/disable/key revocation invalidate authority; grant/key changes require verification within five minutes. Session inventory and explicit revocation; parent session loss also denies delegation. Expiry/clock/revocation and real-browser checks. |
| V8.1.1–4, V8.2.1–4, V8.3.1–3 | Immutable enrollment/signal grants, allowed control paths/intervals and delegation intersections. No field-level or arbitrary tenant ACL. Time/expiry and policy version affect authority; geolocation/device reputation do not. Cross-principal fixtures cover rows, metadata, cursors and mutation publication. |
| V16.1.1, V16.2.1–5, V16.3.1–4 | Access audit stores bounded identity/action/outcome/time/request IDs; no bearer, challenge or telemetry bodies. Denials are rate-bounded with a suppressed-count total. Operational logs separately flow through the companion. Audit attribution, cap and failure fixtures; this is not a promise to retain every attack attempt. |
| V16.4.1–2, V16.5.1–4 | Structured JSON, private files and host-admin trust. Unexpected access failures quarantine authority; API errors redact internal paths. Persistence faults, malformed-state, generic-error and recovery fixtures. Host administrators can alter local files. |

Explicit exclusions and differences:

- V6.1.2 and V6.2 concern passwords; Fabric does not offer human passwords.
  V6.6, V6.8 and V7.6 federation/OTP pathways are absent; local passkeys and
  setup invitations are tested under the rows above. V6.5.7–8 do not establish
  Fabric-controlled biometrics or TOTP: authenticator user verification belongs
  to the WebAuthn implementation/device.
- V3.6.1 has no external runtime asset to protect: both packages contain the
  complete hashed shell. V3.7.2–3 have no automatic external redirect. V3.7.4
  cannot preload every private operator hostname. V3.5.4 relies on deploying
  this application at its dedicated configured origin.
- V3.4.7 violation reporting, V6.3.5/V6.3.7 proactive security notifications,
  V6.4.5 automated renewal reminders, V8.4.2 device-posture/risk analysis and
  V16.4.3 independent remote audit custody are not implemented. Local audit,
  expiry visibility and same-host self-observation do not substitute for them.
  These differences preclude a blanket ASVS claim.
- V8.4.1 general multitenancy is outside the product boundary. Scoped principals
  on one installation still require the cross-principal denial checks above.

This is the explicit console/access selection, not an audit of every ASVS
chapter or of upstream dependencies. Existing delivery/storage checks continue
to own their respective contracts. Record a defect as a regression before
crediting its correction; do not convert a documented mechanism into a pass.
