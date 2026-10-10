# Local access operations

This guide describes the current [access adapter](../crates/fabric-server/src/access.rs),
[HTTP routes](../crates/fabric-server/src/console.rs) and
[command-line client](../src/bin/fabricctl.rs). See the
[identity boundary](architecture/identity-access.md) for assumptions and
[console acceptance protocol](experiments/formal/console-access-protocol.md) for
verification requirements. These instructions do not establish browser or
hardware compatibility beyond recorded tests.

## Configure and enroll the first owner

Use the [server configuration example](../packaging/etc/server.conf.example).
Set `access_origin` to the exact trusted HTTPS origin, including a nondefault
port, and `access_rp_id` to its hostname without scheme or port. The server does
not derive either from request headers. Install a certificate trusted by the
browser and configure `console_dir` with the packaged console assets. Changing
these identity settings after enrollment requires a planned migration.

Start normal serving with `fabric-server serve /etc/fabrico11y/server.conf`.
On the first successful access-store initialization, the server creates
`state_dir/access/access-bootstrap.secret` with mode 0600. Read it locally as
the service owner or OS administrator; use the browser's **First owner setup**
at `https://YOUR_HOST:PORT/console/`. The secret expires after ten minutes and
is consumed when registration succeeds. The server creates the owner ID and
grants; the browser supplies a display name and a passkey with user verification.
New owner, additional-key, invitation and recovery enrollment requires an
authenticator that can store a resident/discoverable credential and perform user
verification. A conforming browser rejects that request if its authenticator
cannot provide the requested capability. Existing credentials remain usable for
explicit principal sign-in. Resident-key browser acceptance is recorded separately from this request
policy; the server cannot prove resident storage from unsigned client output.
Each ceremony expires after five minutes. Save the returned principal ID for
future sign-in and recovery; display names do not identify accounts.

If first setup expires or is interrupted before a passkey is enrolled, stop the
service and run as its filesystem owner:

```sh
fabric-server renew-bootstrap /etc/fabrico11y/server.conf
```

This command obtains the stopped-service lock and renews only an unfinished,
empty first-owner installation. It does not erase an established account or
repair a corrupt access store. Read the replacement protected secret locally.

## Manage humans and explicit grants

In console Settings, reverify with a passkey before changing credentials or
grants. Sensitive mutations require verification within five minutes. Add a
second independent passkey while an existing key is available; the server
refuses to revoke the account's last passkey. Passkey revocation ends affected
sessions. Logout and reauthentication retire the prior browser session;
separate device sessions remain until explicitly revoked, expired or invalidated
by policy changes. Sessions last at most eight hours, expire after thirty
minutes idle and do not survive server restart.

Create a human invitation from Settings with an explicit scope. The once-returned
invitation expires after ten minutes, is consumed when invited enrollment begins,
and is bound to a server-generated principal ID. The recipient enters it in
**Enroll invited passkey**. An interrupted invitation requires a new invitation;
the original secret is not reusable.

Scopes use immutable `enrollment_id` values returned by authorized inventory,
actions, signal classes, query bounds and configuration limits. An empty enrollment
list gives a scoped account no telemetry. `installation_wide: true` is explicit
and may be granted only within the issuer's existing authority. For example,
a read-only workload scope is:

```json
{
  "actions": ["telemetry_read", "inventory_read"],
  "installation_wide": false,
  "enrollments": ["COPY_EXACT_ENROLLMENT_ID_FROM_INVENTORY"],
  "signals": ["metrics"],
  "max_query_window_s": 3600,
  "max_query_rows": 200,
  "allowed_log_paths": [],
  "min_interval_s": 1,
  "max_interval_s": 3600,
  "enrollment_namespace": null,
  "max_enrollments": 0
}
```

Scope changes invalidate previously issued policy authority. Workload issuance
and rotation preserve only the issuing human's cookie when its own grants and
epoch remain unchanged; previously captured request authority stays invalid.
Invitation creation leaves existing policy authority unchanged until enrollment.
Refresh and reverify before another privileged operation. Node-enrollment authority also
requires an allowed namespace and bounded enrollment quota; node configuration
requires permitted log paths and sample intervals.

## Workload clients and rotation

A freshly verified human with `grant_manage` and `identity_manage` may issue a workload credential
from Settings (`POST /v1/console/workloads`, JSON `name`, `scope`, `ttl_s`).
The token is returned once. Workloads cannot administer identities or grants;
ordinary token lifetime is at most thirty days. Store the token in an owner-only
regular file with mode 0600, outside the client config. Do not use a symlink.
Copy the [access client example](../packaging/etc/access.conf.example) and set
its exact HTTPS origin, absolute CA path and absolute `workload_token_file`.

```sh
fabricctl access /home/operator/.config/fabrico11y/access.conf node list
fabricctl access /home/operator/.config/fabrico11y/access.conf node pause source-1
fabricctl access /home/operator/.config/fabrico11y/access.conf node resume source-1
fabricctl access /home/operator/.config/fabrico11y/access.conf node config source-1 --interval 15 --log /var/log/app.log
```

`node add`, `node revoke` and `query '<QUERY_JSON>'` use the same explicit mode
and are admitted only by their grants. Query JSON follows the
[query contract](architecture/query.md). The scoped API caps requests at 1,000
rows and a 24-hour window, with smaller grant bounds applied first. The client
sends `x-fabric-client-version: 1`, refuses redirects and reports HTTP denial
without switching credentials or endpoints.

Rotate from a freshly verified human using Settings or
`POST /v1/console/workloads/PRINCIPAL_ID/rotate` with `{"ttl_s":86400}`.
Rotation preserves the workload ID and limits old-token overlap to at most
600 seconds, or the old token's earlier expiry. Replace the token file before
that overlap ends. Explicit credential revocation is immediate for subsequent
authorization checks.

Delegation (`POST /v1/console/delegations`) names `workload_id`, `scope` and
`ttl_s`. Its scope must fit both the human and the workload credential. It lasts
at most 900 seconds and cannot outlive either exact parent session or credential.
Logout, server restart, parent revocation or policy changes invalidate it.
Delegation does not grant recursive delegation, identity management or enrollment.

## Offline owner recovery

Stop the server and identify the existing human owner's immutable principal ID.
As the service filesystem owner, run:

```sh
fabric-server recover-access /etc/fabrico11y/server.conf OWNER_PRINCIPAL_ID
```

The command refuses an active service lock, validates durable Control state,
and saves bounded protected copies of Control and access files into one of two
`state_dir/owner-recovery-backup-N` slots. If both slots are occupied, export a
prior backup to controlled storage before freeing a slot; the command does not
overwrite it automatically. Backups contain protected identity material and may
contain an old bootstrap secret.

Recovery preserves principal IDs, rotates the installation epoch, invalidates
all workload/delegated credentials and sessions, and removes the recovered
owner's old passkeys. It issues a ten-minute replacement secret in
`state_dir/access/access-bootstrap.secret`; enroll the existing owner through
**First owner setup** and then add a second key. Other humans' stored passkeys
remain, while their sessions and prior policy authority are invalidated.

An interrupted durable recovery transaction rolls forward from its protected
journal on the next open. An interrupted Control mutation is reconciled only by
this explicit offline procedure against validated durable Control evidence;
it is not replayed automatically. Corrupt or missing initialized state fails
closed. Restoring an entire older snapshot is an OS-owner operation: the server
cannot independently detect a matching rollback of all local identity files.
After a restoration, perform explicit owner recovery before resuming remote
access so old installation credentials are replaced.

## Explicit legacy migration

Normal `serve` rejects `/v1/admin/*` shared-master routes. Historical fixture or
migration operation requires the explicit `fabric-server serve-legacy CONFIG`
and `fabricctl admin ADMIN_CONFIG ...` modes. The admin config uses
`admin_token_file`; `fabricctl access` rejects that key and never falls back to
it. Legacy mode is a separate operational mode and does not satisfy authenticated
console release acceptance. Migrate to configured normal serving and issue
scoped credentials before using production clients.
