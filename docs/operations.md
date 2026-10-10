# Operating FabricO11y

This guide covers one central server and remote Linux Spindles. The
[current state](CURRENT.md) records tested artifacts and acceptance limits;
the [release plan](milestones/release-readiness.md) defines the supported matrix.

## Build and install

Build the native binaries and console together using the
[package instructions](../packaging/README.md), Rust 1.99.0 and the resource
launcher. Install the selected, checksum-verified artifact with
`sudo apt install ./PACKAGE.deb` on Debian or `sudo dnf install ./PACKAGE.rpm`
on Fedora. Services remain disabled until configured. The finite acceptance
matrix targets Debian 13 and Fedora 44 x86_64, systemd and unified cgroup v2;
a compatible dependency list alone does not establish another OS's acceptance.

The package installs `fabric-node`, `fabric-server` and `fabricctl` in `/usr/bin`, the units `fabrico11y-node.service` and `fabrico11y-server.service` in `system-fabrico11y.slice`, and a sysusers file that creates the system user and group `fabricolly`. Installation refuses an existing `fabricolly` account that is not a non-login system account with primary group `fabricolly`. Examples are in `/usr/share/doc/fabrico11y/examples`.

## Server

1. Create `/etc/fabrico11y/server.conf` from the example. Provide a TLS certificate and key trusted by both browsers and Spindles. Configure the exact `access_origin`, hostname-only `access_rp_id`, and packaged `console_dir`. The current configuration also requires a legacy admin-token file of at least 32 printable characters; normal serving does not accept that token on its disabled master routes. Make the key and token `0640 root:fabricolly`.
2. `sudo systemctl enable --now fabrico11y-server.service`.
3. Follow [first-owner setup](access-operations.md#configure-and-enroll-the-first-owner) to read the protected local bootstrap secret and enroll a passkey at `https://YOUR_HOST:PORT/console/`. Add a second passkey and save the immutable owner ID for recovery.

Storage settings include `journal_bytes` (20 GiB), `journal_file_bytes` (64 MiB),
`retention_s` (86,400 seconds), `retention_bytes` (100,000,000,000 bytes) and
`seal_workers` (half the CPUs, bounded to one through four by default). Retention
deletes whole sealed Segments oldest first when either age or bytes exceed policy.
Reserve additional bounded space for the journal, sealing work and Spools;
100 GB is the retained-telemetry limit, not a total filesystem ceiling. The
console reports this policy read-only. Apply configuration changes by editing
the server file and restarting the service.

Every production server launch also starts its dedicated `fabric-node` sibling.
Build/install both binaries. Its private token, generated config and persistent
Spool live under `state_dir/self-spindle`; server process logs live under
`state_dir/diagnostics`. The local collector reports as `fabric-server-self` in
the node inventory. It collects both processes' diagnostics and normal host and
output metrics, using the same durable delivery and query paths as edge data.
The server service's existing cgroup limit covers both processes.

For a CA-signed certificate, set `self_spindle_ca` to the CA PEM file. The default
trust input is `tls_cert`, suitable for a trusted self-signed certificate. The
certificate must cover the local listener IP (loopback for a wildcard bind), or
use `self_spindle_url=https://localhost:PORT` with a matching localhost SAN and
loopback listener. The URL cannot redirect the child to a different server.
`self_spindle_executable` optionally selects an absolute `fabric-node` path.
See [deployment](architecture/deployment.md) for shutdown, pause and storage bounds.

Inspect process samples in Explore by selecting logs from `fabric-server-self`
and filtering for `event=process_sample`, within an authorized time window.

These records contain process RSS, high-water RSS and CPU ticks with tick rate.
They are log bodies, not a new generic metrics receiver. A printed listener/PID
confirms process startup; query the received observations to check delivery.

## Scoped client and Spindle controls

In console Settings, issue a workload credential with only the required query
and Spindle-control grants. Store its once-returned token in a regular,
owner-only file with mode 0600. Configure the client:

```text
server_url=https://fabric-server.example:7443
server_ca=/etc/fabrico11y/ca.pem
workload_token_file=/home/operator/.config/fabrico11y/workload-token
```

```sh
fabricctl access access.conf node add web01 --log /var/log/app.log --interval 15
fabricctl access access.conf node list
fabricctl access access.conf node config web01 --log /var/log/app.log --interval 30
fabricctl access access.conf node pause web01
fabricctl access access.conf node resume web01
```

`node add` returns the Spindle token once. Enrollment grants require an explicit
namespace and quota; configuration grants constrain log paths and sample intervals.
The UI exposes the same operations. See [access operations](access-operations.md)
for scope examples, token rotation, delegated agents and recovery.

`fabricctl access access.conf query '<QUERY_JSON>'` supports logs, metric points
and spans with a window of at most 24 hours and at most 1,000 rows, further
restricted by the grant. [Query answers](architecture/retained-history.md) include
completeness, scoped freshness and gaps; follow the opaque `next_page` handle.
Legacy `fabricctl admin` works only with the explicit `serve-legacy` migration
mode. It does not provide the scoped console boundary.

## Node

1. Create `/etc/fabrico11y/node.conf` from the example with `server_url`, `server_ca` and `token_file` (the token printed by `node add`, stored `0640 root:fabricolly`).
2. Give the service read access to each selected log with a narrow ACL, for example `setfacl -m u:fabricolly:r /var/log/app.log`. An unreadable log produces a visible collection gap; the node never escalates.
3. Optionally set `traces_listen=127.0.0.1:4318` so local applications can export traces to the node, with `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT=http://127.0.0.1:4318/v1/traces` and `OTEL_EXPORTER_OTLP_TRACES_PROTOCOL=http/protobuf` in their environment. The node answers an export only after it is in the spool; spans are queried with `{"kind":"spans", ...}` ([retained history](architecture/retained-history.md)).
4. `sudo systemctl enable --now fabrico11y-node.service`.

The node samples host metrics on its interval, reads logs every second, keeps every batch in its spool until the server acknowledges it, and polls for configuration every 5 s. Log paths and interval set through scoped `node config` replace the local file's values once the node has validated and stored them; the spool, its ceiling and the server target stay local. The CLI always adds its own bounded diagnostic source; server-owned nodes also retain the server source. These pins share the sixteen-source ceiling and survive remote replacement. Edge diagnostics go to the existing configured server. `fabricctl inspect /etc/fabrico11y/node.conf` reports the spool: batches, acknowledged sequence, backlog, and any recovery state.

## Recovery states

| What you see | Meaning | What to do |
| --- | --- | --- |
| `interrupted_append=true` in `fabricctl inspect` | A process died during an append or rotation. | Nothing. The next start keeps every committed batch and truncates the incomplete tail. |
| `recovery_required=true`, or `recovery required` at start | A write or sync call on the spool or server journal reported an error. Readable bytes cannot prove the data is durable. | Do not delete the marker. Check the storage device, then move the directory aside. **Node:** starting with an empty spool creates a new node identity and re-reads each selected log from its start, so lines already delivered under the old identity appear again; batches the old spool had not delivered are lost unless it can be read elsewhere. **Server:** acknowledged batches exist only on the server, so restore its state directory from a backup; nodes still hold, and will resend, every batch they have not had acknowledged. |
| `coverage unknown since …` gap in a batch | A cycle could not commit (for example a full spool), or collection was paused. | Informational. The interval named has no data. For a full spool, check server reachability; acknowledged sealed files are deleted automatically. |
| `delivery: … conflict` on the node | The server holds different bytes for the same batch identity. | Stop the node. This means two spools share an identity or a spool was altered; investigate before continuing. |
| `delivery: … gap` on the node | The server has fewer batches for this stream than the node has acknowledged. | The server lost acknowledged data. Restore the server state before continuing. |
| Query answer `complete: false` with `unavailable` entries | A segment could not be read. | Other data is still answered. Check the segment's files against its manifest; restore the segment from a backup if you keep one. |
| HTTP 410 on a page | Retention removed data the page's snapshot covered. | Start the query again from the first page. |

`systemctl stop` requests graceful shutdown: the node stops between cycles, and
the server drains requests and releases its journal. The packaged 30-second stop
deadline bounds shutdown; systemd terminates remaining processes if it expires.
Clients retain unacknowledged Spool data for retry after restart.

## Limits

One operator-controlled installation with operator-trusted TLS and a same-origin
PWA. The Spindle accepts traces through its loopback OTLP/HTTP endpoint; a general
OTLP receiver is outside this release. The offline PWA contains only its public
shell and requires a confirmed online session to display telemetry. Physical
power loss and unrecorded browser/device combinations are not qualified. See
the [product contract](PRODUCT-CONTRACT.md) and [qualification](QUALIFICATION.md).

## Uncertain control publication

If an administrative change reports `control publication uncertain; reopen
required`, or subsequent node requests are refused after a directory-sync error,
resolve the storage error and restart the server to reload `control.json`.
The live instance refuses authentication and further changes because the renamed
state may differ from its previous in-memory inventory. An oversized inventory
update is rejected before publication and leaves the existing state usable.

## Reporting problems

Use the [bug-reporting guide](CONTRIBUTING.md#reporting-bugs) for operational
defects and the [private security channel](../SECURITY.md) for suspected
vulnerabilities. Include the exact package version and checksum with a minimal
synthetic reproduction.
