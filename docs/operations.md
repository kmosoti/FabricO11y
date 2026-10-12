# Operating FabricO11y

This guide covers one central server and remote Linux Spindles. The
[current state](CURRENT.md) records tested artifacts and acceptance limits;
the [release plan](milestones/release-readiness.md) defines the supported matrix.

## Install

Download the `.deb` or `.rpm` and `SHA256SUMS` from the
[v0.1.0-alpha.1 release](https://github.com/kmosoti/FabricO11y/releases/tag/v0.1.0-alpha.1)
(to build from source, see the [package instructions](../packaging/README.md)), then verify and install:

```sh
sha256sum -c --ignore-missing SHA256SUMS
sudo apt install ./fabrico11y_*.deb   # Debian 13
sudo dnf install ./fabrico11y-*.rpm   # Fedora 44
```

Services remain disabled until configured. The finite acceptance
matrix targets Debian 13 and Fedora 44 x86_64, systemd and unified cgroup v2;
a compatible dependency list alone does not establish another OS's acceptance.

The package installs `fabric-node`, `fabric-server` and `fabricctl` in `/usr/bin`, the units `fabrico11y-node.service` and `fabrico11y-server.service` in `system-fabrico11y.slice`, and a sysusers file that creates the system user and group `fabricolly`. Installation refuses an existing `fabricolly` account that is not a non-login system account with primary group `fabricolly`. Examples are in `/usr/share/doc/fabrico11y/examples`.

## Server

1. Create `/etc/fabrico11y/server.conf` from the example. Provide a TLS certificate and key trusted by both browsers and Spindles. Configure the exact `access_origin`, hostname-only `access_rp_id`, and packaged `console_dir`. Normal serving needs no administrator token: `admin_token_file` is read only by the migration mode `serve-legacy`. Make the TLS key `0640 root:fabricolly`. Every token and secret file must be a single-link regular file, not a symlink, owned by the service user or root, with mode `0600` or, if root owns it, `0640`; anything else is refused at start ([security hardening](architecture/security-hardening.md#credential-files)).
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

## Network and certificate setup

Choose a stable server hostname before enrolling passkeys. `access_rp_id` must be a
hostname, not an IP address. Unless DNS resolves it, map it on every VM and browser
host (`echo '192.0.2.10 telemetry.example.com' | sudo tee -a /etc/hosts`). The certificate
SAN must match that name and also cover `localhost` and `127.0.0.1` for the dedicated
local Spindle. To use a private CA, create it and the server certificate on the server:

```sh
HOST=telemetry.example.com
mkdir -m 0700 ~/fabric-pki && cd ~/fabric-pki
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout ca.key -out ca.pem \
  -days 825 -subj "/CN=Fabric CA" -addext basicConstraints=critical,CA:TRUE \
  -addext keyUsage=critical,keyCertSign,cRLSign
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout server.key -out server.csr -subj "/CN=$HOST"
printf 'subjectAltName=DNS:%s,DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n' "$HOST" > server.ext
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out server.pem -days 825 -extfile server.ext
sudo install -m 0644 ca.pem server.pem /etc/fabrico11y/
sudo install -m 0640 -o root -g fabricolly server.key /etc/fabrico11y/server.key
```

Set `self_spindle_ca=/etc/fabrico11y/ca.pem` in `server.conf`. Copy `ca.pem` to each
edge as `/etc/fabrico11y/ca.pem` (`server_ca`) and import it into each browser's
trusted certificate store. Keep `ca.key` on the server or offline; never copy it to an
edge. A browser certificate exception or a disabled TLS check is not a substitute.

Check the configured names and trust before enabling collection:

```sh
openssl verify -CAfile /etc/fabrico11y/ca.pem /etc/fabrico11y/server.pem
openssl x509 -in /etc/fabrico11y/server.pem -noout -ext subjectAltName
```

Bind the central listener to an intended private/LAN/VPN address when possible.
In the host firewall and any cloud firewall, allow its configured TCP port
(7443 in the example) only from the edge and operator networks. For example, with edges and operators on
192.0.2.0/24, Fedora's firewalld:
`sudo firewall-cmd --permanent --add-rich-rule='rule family="ipv4" source address="192.0.2.0/24" port port="7443" protocol="tcp" accept' && sudo firewall-cmd --reload`.
Debian has no firewall by default; with ufw,
`sudo ufw allow proto tcp from 192.0.2.0/24 to any port 7443`. Include IPv6
rules if using an IPv6 listener. Edges initiate outbound HTTPS; they do not need
an inbound Fabric port. The optional OTLP receiver stays on loopback port 4318
and is for applications on that edge. Preserve SSH/VPN administration when
editing existing firewall rules. Packages do not change those rules.

Visit the configured HTTPS origin without a certificate error, complete owner
enrollment, then confirm that the companion appears and its diagnostic records
are queryable. Check remote delivery separately after enrolling an edge; a
listening port alone does not establish certificate trust or durable custody.

## Reader tokens and Spindle controls

The owner enrolls, configures, pauses, resumes and revokes Spindles in the console;
these are owner-only. After revoking an edge, stop its `fabrico11y-node.service` and delete its token file; the server rejects its deliveries, and the node keeps unsent batches in its Spool until you remove it. For scripts and AI agents, issue a read-only reader token in
console Settings and use it with `fabricctl access access.conf node list` and
`query` (logs, metrics and spans examples), as [access operations](access-operations.md#reader-tokens-and-read-only-clients)
describes with token storage, rotation and recovery. A reader can query telemetry (logs,
metric points and spans, at most 24 hours and 1,000 rows) and read status and inventory,
nothing else. [Query answers](architecture/retained-history.md) include completeness,
freshness and gaps; follow the opaque `next_page` handle. Legacy `fabricctl admin`
works only with the explicit `serve-legacy` migration mode and does not provide the
role boundary.

## Node

1. Create `/etc/fabrico11y/node.conf` from the example with `server_url`, `server_ca` and `token_file` (the token shown once at enrollment; the same file rules apply). Install `ca.pem` as `server_ca`. Create the token file (`token_file=/etc/fabrico11y/node-token`), paste the token, press Enter, then Ctrl-D:

```sh
sudo install -m 0640 -o root -g fabricolly /dev/stdin /etc/fabrico11y/node-token
```
2. Give the service read access to each selected log with a narrow ACL, for example `setfacl -m u:fabricolly:r /var/log/app.log` (`sudo apt install acl` on minimal Debian if `setfacl` is missing). An unreadable log produces a visible collection gap; the node never escalates.
3. Optionally set `traces_listen=127.0.0.1:4318` so local applications can export traces to the node, with `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT=http://127.0.0.1:4318/v1/traces` and `OTEL_EXPORTER_OTLP_TRACES_PROTOCOL=http/protobuf` in their environment. The receiver accepts only `application/x-protobuf`, so `curl` with JSON fails; use an OTLP/HTTP-protobuf application.
4. `sudo systemctl enable --now fabrico11y-node.service`.
5. If you set `traces_listen`, emit a test span once the receiver is listening (Debian needs `sudo apt install python3-venv` first):

```sh
python3 -m venv otel && otel/bin/pip install opentelemetry-sdk opentelemetry-exporter-otlp-proto-http
cat > span.py <<'PY'
import socket, time
for _ in range(30):  # wait up to 30 s for the node's receiver
    try: socket.create_connection(('127.0.0.1', 4318), 1).close(); break
    except OSError: time.sleep(1)
else: raise SystemExit('fabric-node is not listening on 127.0.0.1:4318')
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
provider = TracerProvider()
provider.add_span_processor(SimpleSpanProcessor(OTLPSpanExporter()))
with provider.get_tracer("walkthrough").start_as_current_span("walkthrough-span"):
    pass
provider.shutdown()
PY
OTEL_SERVICE_NAME=walkthrough OTEL_EXPORTER_OTLP_TRACES_ENDPOINT=http://127.0.0.1:4318/v1/traces otel/bin/python span.py
```

The node answers an export only after it is in the spool; query spans as in [access operations](access-operations.md#reader-tokens-and-read-only-clients).

The node samples host metrics on its interval, reads logs every second, keeps every batch in its spool until the server acknowledges it, and polls for configuration every 5 s. Log paths and interval set through the console's node configuration replace the local file's values once the node has validated and stored them; the spool, its ceiling and the server target stay local. The CLI always adds its own bounded diagnostic source; server-owned nodes also retain the server source. These pins share the sixteen-source ceiling and survive remote replacement. Edge diagnostics go to the existing configured server. `fabricctl inspect /etc/fabrico11y/node.conf` reports the spool: batches, acknowledged sequence, backlog, and any recovery state.

## Recovery states

| What you see | Meaning | What to do |
| --- | --- | --- |
| `interrupted_append=true` in `fabricctl inspect` | A process died during an append or rotation. | Nothing. The next start keeps every committed batch and truncates the incomplete tail. |
| `recovery_required=true`, or `recovery required` at start | A write or sync call on the spool or server journal reported an error. Readable bytes cannot prove the data is durable. | Do not delete the marker. Check the storage device, then move the directory aside. **Node:** starting with an empty spool creates a new node identity and re-reads each selected log from its start, so lines already delivered under the old identity appear again; batches the old spool had not delivered are lost unless it can be read elsewhere. **Server:** acknowledged batches exist only on the server, so restore its state directory from a backup; nodes still hold, and will resend, every batch they have not had acknowledged. |
| `coverage unknown since …` gap in a batch | A cycle could not commit (for example a full spool), or collection was paused. | Informational. The interval named has no data. For a full spool, check server reachability; acknowledged sealed files are deleted automatically. |
| A start error naming a credential file that "must be a single-link regular file" | A token or secret file is a symlink, has another hard link, belongs to another user or can be read by others. | Replace it with a fresh regular file of mode `0600` (or root-owned `0640`) and restart. |
| `delivery: … conflict` on the node | The server holds different bytes for the same batch identity. | Stop the node. This means two spools share an identity or a spool was altered; investigate before continuing. |
| `delivery: … gap` on the node | The server has fewer batches for this stream than the node has acknowledged. | The server lost acknowledged data. Restore the server state before continuing. |
| Query answer `complete: false` with `unavailable` entries | A segment could not be read. | Other data is still answered. Check the segment's files against its manifest; restore the segment from a backup if you keep one. |
| HTTP 410 on a page | Retention removed data the page's snapshot covered. | Start the query again from the first page. |

`systemctl stop` requests graceful shutdown: the node stops between cycles, and
the server drains requests and releases its journal. The packaged 30-second stop
deadline bounds shutdown; systemd terminates remaining processes if it expires.
Clients retain unacknowledged Spool data for retry after restart.

## Upgrade, backup and removal

Check the candidate's recorded compatibility and package checksum before an
upgrade. Use the server, Spindles, CLI and console from the same tested candidate;
mixed-version compatibility is not established. The tested predecessor fixture
in the [installation record](experiments/formal/installation-release-run-01.md)
is specific evidence, not a promise that arbitrary older state can be upgraded
or that new state can be read by an older executable.

Stop the affected services before copying state or replacing the package:

```sh
sudo systemctl stop fabrico11y-node.service fabrico11y-server.service
```

Back up `/etc/fabrico11y` and the complete `/var/lib/fabrico11y` tree, including
access state, journals, Segments, manifests and every Spool:

```sh
sudo tar -C / -cpf fabrico11y-backup.tar etc/fabrico11y var/lib/fabrico11y
```

Include any configured state/Spool directories and key files outside those defaults.
Keep the archive in controlled storage; it holds credentials. To restore, stop the
services, move the current trees aside so no newer file survives, and extract:

```sh
sudo systemctl stop fabrico11y-node.service fabrico11y-server.service
sudo mv /etc/fabrico11y /etc/fabrico11y.pre-restore
sudo mv /var/lib/fabrico11y /var/lib/fabrico11y.pre-restore
sudo tar -C / -xpf fabrico11y-backup.tar
```

A restored snapshot can bring back revoked passkeys and reader tokens, and the server
cannot detect the rollback. On the central server, always run
[offline owner recovery](access-operations.md#offline-owner-recovery) before the first
start, then start the service and enroll the owner again; on an edge, start
`fabrico11y-node.service`:

```sh
sudo -u fabricolly fabric-server recover-access /etc/fabrico11y/server.conf OWNER_PRINCIPAL_ID
sudo systemctl start fabrico11y-server.service
```

Do not run a copied Spool alongside the original: both would share an identity.
A copied configuration alone cannot recover acknowledged telemetry.

Install the checksum-verified candidate as in [Install](#install). The package reloads unit
definitions and preserves operator configuration; it does not perform the
operator's service restart. Explicitly start the roles configured on that host.
For a central-only host, start `fabrico11y-server.service`; it starts its own
dedicated Spindle. An edge starts `fabrico11y-node.service`. Confirm service status,
fresh login, exact retained queries and drained pending Spools. Server restart
invalidates browser sessions; reader tokens persist until they expire or are revoked. The PWA may request a
reload to activate its new coherent shell.

To remove executables while retaining state, use `sudo apt remove fabrico11y`
or `sudo dnf remove fabrico11y`. Both stop the services and preserve configuration,
telemetry and the service account. Verify that both services are inactive.

A deliberate reset destroys retained telemetry, credentials and pending Spool
data. After stopping/removing the services and deciding what backup to retain,
Debian's `sudo apt purge fabrico11y` deletes the default `/etc/fabrico11y` and
`/var/lib/fabrico11y` trees. RPM removal has no purge operation; for the same reset run
`sudo dnf remove fabrico11y && sudo rm -rf /etc/fabrico11y /var/lib/fabrico11y`.
The `fabricolly` account remains. Custom paths are outside that default cleanup. Reinstallation with
empty state requires fresh owner and edge enrollment. Restoring an older access
snapshot requires the [offline owner recovery](access-operations.md#offline-owner-recovery)
procedure before reopening remote access; package rollback alone does not rotate
old credentials or restore lost telemetry.

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
