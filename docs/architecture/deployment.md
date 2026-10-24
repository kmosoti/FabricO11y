# Linux deployment

Split-host qualification's owner fixture preserves its native guest TLS endpoint
through a guest-only DNAT mapping to an owned ephemeral G15 SSH listener. The
execution host's enforcing SELinux policy remains intact. The
[placement protocol](../experiments/formal/release-placement-protocol.md) records
this harness boundary; it changes no installed production service behavior.

Status: accepted contract. The units, slice, sysusers file and Debian/RPM package tooling exist in [packaging/](../../packaging/). [Installation run 02](../experiments/formal/installation-acceptance-run-02.md) passed all seventeen checks and rejected three mutation controls in a Debian 13 unified-cgroup VM. Its result belongs to that package snapshot; the [release plan](../milestones/release-readiness.md) requires fresh acceptance of the deliverable. Historical [static checks](../experiments/formal/alpha-phase5-packaging-static.md) and the [inconclusive legacy-cgroup run](../experiments/formal/installation-acceptance-run-01.md) retain their outcomes. The [product contract](../PRODUCT-CONTRACT.md#linux-installation-contract) owns the installed defaults and acceptance gates. Exact frozen `a5dbda7a` packages subsequently passed fresh Debian 13/Fedora 44 baseline and upgrade/reboot/removal cells; all six named controls were detected ([installation run 03](../experiments/formal/installation-release-run-03.md)). These installation/migration fixtures use explicit legacy administration and do not establish authenticated-console or deployment qualification.

Debian dependencies come from `dpkg-shlibdeps` plus `systemd (>= 249)`;
[check-glibc.sh](../../packaging/check-glibc.sh) refuses binaries requiring symbols
newer than glibc 2.34. The RPM builder declares its corresponding native runtime
requirements and preserves SELinux enforcement. Both include OpenSSL-dependent
native binaries, a hash-verified console and license notices. The selected
x86_64 release matrix is Debian 13 and Fedora 44; dependency compatibility alone
does not qualify other distribution versions.

The package retains a standalone edge Spindle (`fabrico11y-node.service`) and the central server (`fabrico11y-server.service`). Every production server CLI launch additionally owns one dedicated `fabric-node` child ([ADR-0026](../decisions/ADR-0026-launch-a-dedicated-spindle-with-each-server.md)). Both services use the static `fabricolly` user and primary group. The child inherits the server's identity, sandbox and cgroup; it needs no additional service. `fabricctl` is an operator CLI. The two services belong to `system-fabrico11y.slice`, a child of `system.slice`. Existing numeric limits remain unchanged: server `MemoryHigh=2560M`, `MemoryMax=3072M`, `TasksMax=512` now cover server and child together; standalone node `128M`, `256M`, `128`, and aggregate slice `2816M`, `3328M`, `640` retain their meanings. Neither daemon manages or delegates cgroups. [systemd resource control](https://manpages.debian.org/trixie/systemd/systemd.resource-control.5.en.html) defines the slice hierarchy and effective limits.

The production [`fabric-server serve`](../../crates/fabric-server/src/main.rs) supervisor provisions the ordinary control-plane node `fabric-server-self`. Its credential, generated configuration and persistent Spool live beneath `state_dir/self-spindle`; restarts reuse the credential and Strand identity. Missing credentials for an enrolled node, mismatches and revocation require explicit operator recovery. The private process lease rejects another supervisor for the same state. The [`fabric_server::serve`](../../crates/fabric-server/src/lib.rs) library primitive remains available without implicitly launching a process.

After the server binds, the child delivers over verified HTTPS to that same listener. Its managed `--server-log` startup requires a verified, authenticated configuration poll before entering the run loop; a handshake error terminates the child and makes the supervisor stop serving. The standalone edge node retains its configured destination and ordinary retry behavior. A wildcard IPv4/IPv6 bind uses the corresponding loopback IP as destination. The default trust input is `tls_cert`; a CA-signed certificate may need `self_spindle_ca`, and a certificate with a `localhost` SAN may need `self_spindle_url=https://localhost:PORT`. An override must still identify the same local listener and port. The default child executable is `fabric-node` beside `fabric-server`, with `self_spindle_executable` available for an explicit absolute path. [server.conf.example](../../packaging/etc/server.conf.example) shows these options. The printed listening address and PID report a bound listener and spawned process; actual queried diagnostic records establish that observation reached storage.

The server writes `state_dir/diagnostics/server.log`; each production node writes `spool_dir/diagnostics/spindle.log`. Each process retains at most an active 256 KiB file and one 256 KiB rotation, with fixed state events and fifteen-second process samples. The server-owned child collects both active files through ordinary log cursors, Spool commit and ACK. These local paths remain pinned across remote log-path replacement and use two of the existing sixteen source slots. The generated child profile uses a 64 MiB Spool, 64 KiB/s delivery cap and fifteen-second metric interval. Diagnostics emit no record per Batch or ACK, preventing collection from feeding itself. Rotation and unavailable collection can leave gaps before commit; final stop markers may await the next launch. [OperationalLog](../../crates/fabric-adapter-linux/src/operational_log.rs) owns file bounds; the [Spindle view](spindle.md) owns custody and source semantics.

Administrator pause stops collection for the dedicated node just as for an edge node. Local process logs keep their fixed sampling interval and file bounds; new records await collection after resume. Already committed backlog retains existing delivery behavior. Source pinning does not bypass administrative pause.

Unexpected child exit makes the production supervisor stop serving. During normal stop it signals and reaps the child while HTTP remains available, with a twelve-second child deadline followed by up to ten seconds of HTTP graceful shutdown. The server unit's `KillMode=mixed` delivers initial SIGTERM to the supervisor and retains systemd's cgroup-wide kill fallback at its unchanged thirty-second stop timeout. Parent-death signaling asks the child to stop after abrupt parent exit. The [self-observation protocol](../experiments/formal/server-self-observation-protocol.md) registers delivery, restart, child ownership and failure checks; source inspection alone does not establish deployment qualification.

The vendor sysusers declaration owns identity creation, with administrator overrides in `/etc`; installation must reject an existing identity that fails the expected service-account checks. systemd owns runtime/state directories and cgroups. Root owns installed binaries, units and base configuration; the services get only required state and credential access. Narrow local ACLs authorize selected log files. The node reads host `/proc`, `/sys`, and `/sys/fs/cgroup` for the required host metrics, while neither daemon writes cgroup files or gains privileges on source failure. [sysusers.d](https://manpages.debian.org/trixie/systemd/sysusers.d.5.en.html) and [systemd.exec](https://manpages.debian.org/trixie/systemd/systemd.exec.5.en.html) define these mechanisms.

At install acceptance, check real effective resource controls and source visibility under the service sandbox, plus account collisions, directory modes, TLS, restart and retained data. A static unit check is only an early syntax gate. [ADR-0010](../decisions/ADR-0010-use-static-systemd-services-for-alpha.md) records why this is the deployment boundary of the first profile.

The [fixed-query caller checkpoint](../experiments/formal/query-caller-run-01.md)
uses the unchanged packaged companion and installation-wide reader role. Its
disposable smoke verifies independent companion corpus handling; full service
and deployment gates remain separate. Component ownership and deployment
boundaries are unchanged.

The prospective [R3 soak](../experiments/benchmarks/soak-protocol-r3.md) separates
measurement, stopped raw checkpoint and later grading. Its
[observation checks](../../tools/qualification/soak_exposure.py) consume caller
observations and independently pinned expected inputs; they neither observe
elapsed exposure nor authenticate that admission record. The checkpoint uses a
same-filesystem no-replace rename, retaining uncertain state on failure, and the
preservation guard refuses decoded mounts before inventory and first unlink.
The held workload hierarchy remains readable while the guardian records aggregate
accounting and its sole outer process; independent closeout then confirms whole
service stop. Live S1/FULL integration verification remains pending; these mechanisms do not establish
native soak or deployment acceptance and change no packaged service limit.
The [fresh-owner guide caller](../../tools/qualification/owner_walkthrough.py)
uses the native package managers and normal production console in two disposable
guests. It archives guide command spans, protected raw observations and original
stopped journal bytes; the existing independent framing and query oracles grade
the answers. Its phase table bounds accumulated time, rather than replacing the
guide's call sequence. Native execution remains subject to the
[walkthrough protocol](../experiments/formal/owner-walkthrough-protocol.md);
source controls alone establish no deployment acceptance. This is qualification
tooling outside the runtime boundaries shown above.

The supervised release, bridge smoke, native cleanup, Firefox trust and owner
walkthrough callers pass the same explicitly admitted native browser-entry
helper path and SHA-256 into the shared browser launch boundary. The owner
walkthrough binds the pair to its admission and receipt; the native storage
census checks its exact command shape and canonical DATA path without opening
the helper. CandidateFixture copies the verified helper into a fresh owned
scratch directory, passes that immutable snapshot to ConsoleBridge, and includes
its digest in receipt and closeout checks. Replacing the original caller path
after snapshotting does not change the binary selected for launch. These harness
controls do not alter browser predicates or establish acceptance, and their
final source verification remains pending.

The browser fixture's readiness predicate receives the current process explicitly
at initial startup, offline owner recovery and shell restart. The shared helper
retains TLS health polling, dead-process refusal and the existing bounded wait.
This avoids both an unavailable nested function and a stale initial process
handle; it does not replace the subsequent browser/recovery assertions. Its
[regression protocol](../experiments/formal/browser-supervisor-protocol.md)
preserves the original failed packaged run and distinguishes source checks from
a fresh native retry.

The [signed browser security fixture](../experiments/formal/console-access-protocol.md#prospective-signed-browser-security-supplement)
exports only a newly created virtual authenticator key into protected, owned
scratch. It signs and independently verifies malformed-origin/RP/UV assertions
without changing packaged product state. Private keys, ceremonies and cookies
are excluded from public receipts; existing owned-group cleanup applies.

The [announcement caller](../../tools/ui/browser_announcements.py) keeps its
interactive browser/display/AT subtree in private PID/mount/IPC namespaces and
DATA-backed settings, buses and file/null audio. It shares only the existing
fixture network with the genuine TLS console server. AT document snapshots bind
identity; actual events, Orca/Speech Dispatcher utterances and espeak PCM growth
remain separate action evidence. Its dependency-ordered stop precedes the
existing authoritative browser cgroup teardown. This is acceptance tooling,
with native execution pending, and adds no runtime product component.

The qualification-only [native custody caller](../experiments/formal/installation-native-custody-caller-card.md)
observes the installed edge Spool in place with both units and every observed
writer stopped. Complete labelled archives retain original guest metadata;
host copies never become the installed-root authority. Four stopped checkpoints
separate unchanged native installation, candidate drain and actual reboot.
Full query pages and exact payload ACKs join independently observed candidate
processes and boot identities. This harness adds no product dependency or service
and has no executed native acceptance result yet.
