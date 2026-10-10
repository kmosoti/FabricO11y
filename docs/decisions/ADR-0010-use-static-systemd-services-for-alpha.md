# ADR-0010: Use static systemd services for the Linux alpha

## Status

Accepted for the planned alpha; not implemented or installation tested.

Later [ADR-0026](ADR-0026-launch-a-dedicated-spindle-with-each-server.md)
supplements the process topology: the production server CLI owns a dedicated
Spindle child inside the server service's existing cgroup. The two installed
units, static service identity and unchanged numeric limits remain; the
standalone node unit still serves edge collection. This addition does not
retroactively change the installation evidence recorded here.

## Context

One operator-controlled Debian 13/WSL2 host needs two long-running services with durable local state, host metric read access, bounded resources and a reviewable Unix privilege boundary.

## Decision

Install one static `fabricolly` system user and primary group through vendor `sysusers.d`, after verifying any existing account is the expected non-login service identity. Run the node and server in separate systemd units with that explicit identity and separate managed state/runtime paths. Place both under `system-fabrico11y.slice` below `system.slice` for an aggregate bound, with service-specific bounds. systemd owns cgroups; services use read-only cgroup access and no capabilities. The [alpha contract](../PRODUCT-CONTRACT.md#linux-installation-contract) owns exact directives, numeric defaults and phase-5 tests. [sysusers.d](https://manpages.debian.org/trixie/systemd/sysusers.d.5.en.html), [systemd.exec](https://manpages.debian.org/trixie/systemd/systemd.exec.5.en.html), [systemd.service](https://manpages.debian.org/trixie/systemd/systemd.service.5.en.html) and [resource control](https://manpages.debian.org/trixie/systemd/systemd.resource-control.5.en.html) are the systemd 257 references for the target Debian 13 family.

## Alternatives considered

- Dynamic users for these stateful alpha services complicate stable state and local log ACLs.
- A privileged helper, polkit broker, or daemon-owned cgroup manager adds an unused security and lifecycle boundary.
- Two services without a shared slice lack one systemd aggregate bound.

## Evidence

This is an accepted deployment design, not performance or installation evidence. The current WSL host reportedly has systemd 257 and a unified hierarchy, but no alpha service installation has run.

## Consequences

Both services share a Unix principal, so filesystem permissions alone do not isolate their state from each other; separate service paths and application credential boundaries remain necessary. Aggregate and per-service limits can be inspected through systemd. Host source access remains subject to ordinary permissions and explicit log ACLs. No speculative helper or extra service is required.

## Validation

Run the phase-5 installation acceptance in the [alpha contract](../PRODUCT-CONTRACT.md#linux-installation-contract), including actual identity, resource, source-read, failure and restart checks. Refuse promotion when a required runtime check is unrun.

## Related

[Deployment view](../architecture/deployment.md), [current system](../architecture/system.md), [phase ledger](../QUALIFICATION.md#capability-ledger).
