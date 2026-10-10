# Reporting security vulnerabilities

Report suspected authentication bypass, unauthorized telemetry access, credential
exposure or other security defects through
[GitHub's private vulnerability reporting](https://github.com/kmosoti/FabricO11y/security/advisories/new).
This sends the report privately to the repository's maintainers. Do not disclose
exploit details in a public issue.

Include the affected commit or package version and checksum, deployment and
browser versions, the expected access boundary, observed behavior, and minimal
reproduction steps using synthetic data. Describe the permissions an attacker
needs. Redact tokens, cookies, private keys and real telemetry; maintainers can
arrange any additional evidence through the private report.

If GitHub's private form is unavailable, open a public issue requesting a private
security contact without including vulnerability details.

FabricO11y has no tagged release or supported release series yet. Candidate
testing and the current security boundaries are recorded in
[CURRENT.md](docs/CURRENT.md) and the
[security verification map](docs/formal/console-security-map.md). These finite
checks are not a security certification. There is no guaranteed response time
or bug bounty.

For ordinary defects and usability feedback, use the
[bug-reporting instructions](docs/CONTRIBUTING.md#reporting-bugs).
