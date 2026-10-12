# Security hardening

The controls the code enforces at its input boundaries. The role and credential boundary is in [identity and access](identity-access.md); the checks behind each control are finite and named in the [verification matrix](../formal/verification-matrix.md). This page is neither a security audit nor deployment qualification. The original review specification is a [historical record](../experiments/security-hardening/PUBLICATION.md).

## Console peer admission

[peer_admission.rs](../../crates/fabric-server/src/peer_admission.rs) wraps the native TLS console router. The peer is Axum's `ConnectInfo<SocketAddr>`: forwarding headers and ports are ignored, IPv4-mapped IPv6 folds to IPv4 and native IPv6 groups by /64. A missing transport identity answers 503, and a proxy or NAT is one peer.

| Lane | Routes | Rate per peer | Burst | Live per peer | Live overall |
| --- | --- | --- | --- | --- | --- |
| Public authentication | `/v1/console/auth/*` | 2/s | 4 | 2 | 8 |
| Private console | other `/v1/console/*` | 8/s | 8 | 4 | 16 |

The table holds 1,024 peers. A live or indebted entry is never evicted; refilled idle ones are reclaimed after 120 seconds. A /48 tracks at most 64 (`PEERS / 16`) /64 entries; further /64s from that network share one /48 entry and its single budget, so one routed network cannot fill the table. A peer rejection (429) comes before the console's global rate budgets are spent, poisoned bookkeeping answers 503, and the permit is held through the denial audit. This bounds one peer's starvation of others, not a distributed attacker (many /48s or IPv4 addresses can still fill the table), peers behind one address, TLS overload before routing or local users.

## Poisoned locks

Handlers take access, control, cursor, budget and peer-book mutexes through the `lock!` macro in [lib.rs](../../crates/fabric-server/src/lib.rs). A mutex poisoned by a panic under its guard answers 503 JSON instead of panicking on every later request. It is never recovered, because the state it guards is authority. Nodes retry a 503. The read catalog, which holds only derived cache state, still recovers a poisoned guard.

## Local OTLP transport

The [OTLP receiver](../../src/spindle/otlp.rs) accepts loopback addresses only, in the configuration parser and in `start`. It serves 32 connections (a slot is released on spawn failure and unwinding) and queues 64 exports with `try_send`, so a full queue rejects without blocking. The socket adapter reapplies the remaining absolute deadline before every read and write: header 5 s, body 10 s, commit wait 30 s, response write 5 s, request 50 s, connection 120 s, at most 128 requests per connection. HTTP 200 follows only a durable Spool commit; an export may still commit after its waiter timed out. A reconnecting local attacker or a stalled OS can still defeat liveness.

## Selected log paths

[Secure opening](../../crates/fabric-adapter-linux/src/log_source/secure_open.rs) walks from an owned root directory descriptor with one `openat` lookup per component. Intermediate descriptors use `O_PATH`, close-on-exec and no-follow; descriptor metadata refuses symlinks (including magic links) and nondirectories. Each lookup uses the checked parent descriptor, so renaming its path cannot redirect the next lookup. The leaf opens read-only, close-on-exec, no-follow and nonblocking, and must be regular before any read. Rust `File` ownership closes previous directories and all error paths; at most two descriptors coexist.

Relative paths, parent traversal, NULs, paths longer than 4096 bytes and trailing directory syntax are rejected. No syscall error triggers ordinary opening or path canonicalization. An operator who relied on a symlink selects the physical path. This does not authenticate file provenance or stop hard links, regular rotation, mount manipulation or a compromised OS owner.

The [registered amendment](../experiments/formal/service-log-opening-protocol.md) replaces mandatory `openat2`, which failed under required `RestrictSUIDSGID=yes` in the [frozen-candidate installation trial](https://github.com/kmosoti/FabricO11y/wiki/Experiment-formal-installation-release-run-02). A native child-process regression reproduces that denial, then checks exact reads and continued link refusal. It does not qualify rebuilt packages.

## Credential files

One policy covers every token and secret file the Spindle (`sender::open_secret`, also used by `fabricctl`) and the server and its companion (`config::read_secret`) read: a single-link regular file, not a symlink, owned by the caller or root, readable only by its owner (`0600`) or, when root owns it, also by its group (the packaged `0640 root:fabricolly`). Anything else is refused. Only `serve-legacy` reads `admin_token_file`; normal `serve` needs no administrator token.

## Service sandbox

Both units drop all capabilities and set `NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome`, `PrivateTmp`, kernel module, tunable, cgroup, clock and log protection, `LockPersonality`, `MemoryDenyWriteExecute`, `RestrictNamespaces`, `RestrictRealtime`, `RestrictSUIDSGID`, `SystemCallArchitectures=native` and `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6` ([server unit](../../packaging/systemd/fabrico11y-server.service), [contract](../PRODUCT-CONTRACT.md#linux-installation-contract)). Host `/proc`, `/sys` and cgroup reads stay visible for metrics. The frozen-candidate Debian upgrade failed at companion startup; the opening repair preserves these directives. Fresh [installed-system acceptance](https://github.com/kmosoti/FabricO11y/issues/41) on rebuilt exact packages is pending.

## Fail-closed state and supply chain

- Audit denials have their own 256-event cap, so a flood cannot evict a login or control record ([identity and access](identity-access.md#revocation-persistence-and-bounds)).
- A durable server group that disagrees with replay quarantines the commit thread, which answers `Unavailable` until restart; the flag is in memory and replay rebuilds the stream table ([storage](storage.md#failure-behavior-and-assumptions)).
- Workflow actions are pinned by commit SHA, the fuzz nightly by date, and `tools/ci/check-vendor.py` checks the [vendored crates](../../vendor/README.md) against their crates.io archives ([CI notes](../../tools/ci/README.md)).
