# Reporting security vulnerabilities

Report suspected authentication bypass, unauthorized telemetry access, credential
exposure or other security defects through
[GitHub's private vulnerability reporting](https://github.com/kmosoti/FabricO11y/security/advisories/new).
This sends the report privately to the repository's maintainers. Do not disclose
exploit details in a public issue. If the private form is unavailable, open a
public issue requesting a private security contact, without vulnerability details.

Include the affected commit or package version and checksum, deployment and
browser versions, the expected access boundary, observed behavior, and minimal
reproduction steps using synthetic data. Describe the permissions an attacker
needs. Redact tokens, cookies, private keys and real telemetry; maintainers can
arrange any additional evidence through the private report.

## Scope

In scope: the shipped `fabric-server`, `fabric-node` (the Spindle), the console PWA
and `fabricctl`, run in the supported configuration of the
[operations guide](docs/operations.md): normal `serve` with HTTPS and passkeys, and
the packaged service units. The boundary they defend (roles, credentials and what
the host administrator is trusted with) is in
[identity and access](docs/architecture/identity-access.md); the
[security verification map](docs/formal/console-security-map.md) lists the controls
checked.

Out of scope: `serve-legacy` (a migration mode with a shared administrator token), a
compromised host or operating-system administrator, distributed denial of service,
and the [research archive](https://github.com/kmosoti/FabricO11y-research).
Third-party dependency defects belong upstream unless FabricO11y uses them unsafely.

## Supported version

Only the latest `v0.1.0-alpha` pre-release tag is supported, and nothing older gets
fixes. Until the first tag is published, report against the default branch.

## Response and disclosure

- **Acknowledgement:** we aim to acknowledge a report within 7 days. This is a goal,
  not a guarantee.
- **Coordinated disclosure:** please keep details private until a fix is released or
  90 days after the report, whichever comes first. We will agree a shorter or longer
  window with you when a fix needs it or the issue is already being exploited.
- **Credit:** reporters are credited in the advisory and release notes unless they
  ask to stay anonymous.
- **Safe harbor:** we will not pursue or support legal action over good-faith
  research under this policy: testing an installation you own or may test, with
  synthetic data, stopping at proof of access, and avoiding privacy harm, data loss
  and service disruption. This does not authorize testing systems you do not own.

There is no bug bounty and no service-level agreement. The checks recorded in
[CURRENT.md](docs/CURRENT.md) are finite and are not a security certification.

For ordinary defects and usability feedback, use the
[bug-reporting instructions](docs/CONTRIBUTING.md#reporting-bugs).
