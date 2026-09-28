# Operating the Fabric O11y alpha

This guide is for an operator of one controlled Linux installation. It describes what the code does today; the [phase ledger](ALPHA.md#phase-ledger) says which gates have passed. The package's running-install acceptance has **not** been run, so treat installation as untested.

## Build and install

```sh
cargo fetch --locked                      # once, on a machine with network access
packaging/build-deb.sh target/package-out # needs rustc 1.98.0 and dpkg-deb
sudo apt install ./target/package-out/fabrico11y_0.1.0~alpha.1_amd64.deb
```

The package installs `fabric-node`, `fabric-server` and `fabricctl` in `/usr/bin`, the units `fabrico11y-node.service` and `fabrico11y-server.service` in `system-fabrico11y.slice`, and a sysusers file that creates the system user and group `fabricolly`. Installation refuses an existing `fabricolly` account that is not a non-login system account with primary group `fabricolly`. Examples are in `/usr/share/doc/fabrico11y/examples`.

## Server

1. Create `/etc/fabrico11y/server.conf` from the example. Provide a TLS certificate and key signed by a CA your nodes trust, and an admin token of at least 32 printable characters. Make the key and token `0640 root:fabricolly`.
2. `sudo systemctl enable --now fabrico11y-server.service`.

Keys: `listen`, `tls_cert`, `tls_key`, `state_dir`, `admin_token_file`, and optionally `journal_bytes` (default 20 GiB), `journal_file_bytes` (64 MiB; the unit sealed into a segment), `retention_s` (86,400) and `retention_bytes` (20 GiB). Retention keeps at most the age and at most the bytes given, deleting whole segments oldest first.

## Admin client

Create an admin client file readable only by the operator:

```text
server_url=https://fabric-server.example:7443
server_ca=/etc/fabrico11y/ca.pem
admin_token_file=/etc/fabrico11y/admin-token
```

```sh
fabricctl admin admin.conf node add web01 --log /var/log/app.log --interval 15   # prints the node token once
fabricctl admin admin.conf node list
fabricctl admin admin.conf node config web01 --log /var/log/app.log --log /var/log/other.log --interval 30
fabricctl admin admin.conf node pause web01      # resume, revoke likewise
fabricctl admin admin.conf query '{"kind":"logs","node":"web01","from_ns":0,"to_ns":9000000000000000000,"contains":"error","limit":100}'
```

The query kinds, fields and answer envelope are defined in [retained history](architecture/retained-history.md). Answers report `complete`, the retained window, per-node freshness and collection gaps; follow `next_page` for more rows.

## Node

1. Create `/etc/fabrico11y/node.conf` from the example with `server_url`, `server_ca` and `token_file` (the token printed by `node add`, stored `0640 root:fabricolly`).
2. Give the service read access to each selected log with a narrow ACL, for example `setfacl -m u:fabricolly:r /var/log/app.log`. An unreadable log produces a visible collection gap; the node never escalates.
3. `sudo systemctl enable --now fabrico11y-node.service`.

The node samples host metrics on its interval, reads logs every second, keeps every batch in its spool until the server acknowledges it, and polls for configuration every 5 s. Log paths and interval set through `fabricctl admin ... node config` replace the local file's values once the node has validated and stored them; the spool, its ceiling and the server target stay local. `fabricctl inspect /etc/fabrico11y/node.conf` reports the spool: batches, acknowledged sequence, backlog, and any recovery state.

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

Stopping either service with `systemctl stop` finishes the work in progress: the node stops between cycles, and the server answers every in-flight request and releases its journal before exiting.

## Limits

One operator-controlled installation, TLS with operator-provided certificates, no UI and no general OTLP receiver. Physical power loss has not been tested. See the [alpha contract](ALPHA.md) for the qualification gates and their status.
