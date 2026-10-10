# Bounded input security hardening

This change adds runtime controls for three reviewed input boundaries and repairs
contained verification setup. It is not a completed security audit or deployment
qualification. The original review base is
`18f6b367d0f34886847ea25b9168e50eb0425300`.

## Console peer admission

The native TLS composition installs [peer admission](../../crates/fabric-server/src/peer_admission.rs)
outside the existing console router. It obtains `ConnectInfo<SocketAddr>` from
Axum's accepted connection, ignores forwarding headers and source ports,
normalizes IPv4-mapped IPv6, and groups native IPv6 peers by /64. Missing transport
identity fails closed. A proxy or NAT is one peer; no forwarded-address trust is
implicitly enabled.

The public authentication lane has a per-peer token bucket of 2 requests/second,
burst 4, and two live requests per peer. The private console lane has 8/second,
burst 8, and four live requests per peer. Outer global live limits are eight and
sixteen respectively. Peer entries are bounded at 1,024; an entry cannot be
reclaimed while live or indebted. Fully refilled inactive entries become eligible
after 120 seconds. Rejections do not log arbitrary peer/credential/path data.

This deliberately uses a smaller composition than replacing console authorization:
the existing global fixed-second rate limits, credential checks, CSRF, freshness,
body limits and query-worker permits remain unchanged. Peer rejection happens
before those global rate budgets are spent. The outer request guard remains held
through authorization-denial auditing. Token credit uses monotonic time and
saturating integer arithmetic; poisoned bookkeeping fails closed.

These limits mitigate a single distinct peer's starvation behavior, not distributed
attacks, traffic from shared peer identities, pre-routing TLS overload, or hostile
local users. Synchronous access persistence is not made preemptible by an async
request timer. Tests of the private router without the production wrapper do not
establish that transport peer integration works; native TLS/browser checks remain
required.

## Local OTLP transport

The [OTLP receiver](../../src/spindle/otlp.rs) retains 32 simultaneous connections,
a 64-export queue, its existing body/header size ceilings, and durable Spool
confirmation before HTTP 200. A connection owns its slot through an RAII guard,
including spawn failure and unwinding. Direct `start` rejects non-loopback bind
addresses as well as the configuration parser.

The socket adapter sits beneath `BufReader` and reapplies the remaining absolute
deadline before every read/write. Header reads have five seconds, body reads ten,
commit waits thirty, and response writes five; each is clipped by a fifty-second
request and a 120-second accepted-connection lifetime. A connection serves at
most 128 requests. Already-buffered requests still check these bounds.

Queue submission uses `try_send`. A full queue rejects without blocking before
the commit timer. An enqueued export may still commit after its HTTP waiter times
out; no timeout is interpreted as proof of either a commit or a rollback. Only
observed durable-success confirmation yields 200. These bounds do not guarantee
liveness against an endlessly reconnecting local attacker or a stalled OS.

## Selected log paths

[Secure opening](../../crates/fabric-adapter-linux/src/log_source/secure_open.rs)
uses Linux `openat2` with all-component `RESOLVE_NO_SYMLINKS` and
`RESOLVE_NO_MAGICLINKS`, read-only, close-on-exec, no-follow and nonblocking flags.
The returned descriptor is checked as a regular file and used for metadata,
Btrfs identity, prefix verification and data reads. Backlog inspection and
companion startup use the same helper. Relative paths, parent traversal and NULs
are rejected; an unavailable syscall or blocked policy never falls back to
ordinary pathname opening.

Operators must explicitly select and grant a physical absolute path when an old
source passed through a symlink. This can include distribution directory aliases.
Unreadable sources remain visible failures and must not advance unseen cursors.
No-symlink resolution does not authenticate file provenance or stop hardlinks,
ordinary rotation, mount manipulation or a compromised OS owner.

## Verification status and remaining work

The CI commit adds explicit rustup self-update policy under the actual relocated
home, validates selected compiler locations/version, tests home validation, and
requires both a successful Kani canary and a named reachable-assertion negative
canary. It does not skip or weaken the extended registry. On the first CI-only
commit, hosted Actions passed toolchain preflight, Kani setup and both canaries;
that result is not runtime verification of the subsequent code changes.

New unit and synthetic-loopback tests cover peer normalization/accounting,
symlink/refusal/descriptor behavior, queue-full rejection, deadlines, late replies,
request caps and connection-slot ownership. They require execution on the matching
implementation revision. The drafting environment has no Rust compiler or the
repository's required mounted/systemd test environment, so no local application
execution is claimed. Full fast/extended/browser gates, repeated performance
measurements, and the complete specification's independent negative-control
matrix remain required before declaring the work fully verified.

Use the existing resource launcher for project workloads. Preserve actual failed
runs and their revision identities. See [identity and access](identity-access.md),
[Spindle architecture](spindle.md), and [deployment](deployment.md) for the wider
contracts.
