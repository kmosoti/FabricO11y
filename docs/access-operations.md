# Local access operations

This guide describes the current [access adapter](../crates/fabric-server/src/access.rs),
[HTTP routes](../crates/fabric-server/src/console.rs) and
[command-line client](../src/bin/fabricctl.rs). See the
[identity boundary](architecture/identity-access.md) for assumptions and
[console acceptance protocol](experiments/formal/console-access-protocol.md) for
verification requirements. There are three roles ([ADR-0029](decisions/ADR-0029-use-three-fixed-roles-for-the-first-release.md)):
the owner (this guide's human), read-only reader tokens for scripts and AI agents, and
Spindles. These instructions do not establish browser or hardware compatibility beyond
recorded tests.

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
is consumed when registration succeeds. The server creates the owner ID; the
browser supplies a display name and a passkey with user verification.
Enrollment of the owner, an additional key and recovery requires an authenticator
that can store a resident/discoverable credential and perform user verification;
a conforming browser rejects the request if it cannot. The server cannot prove
resident storage from unsigned client output, so browser acceptance records it
separately. Each ceremony expires after five minutes. Save the returned principal
ID for future sign-in and recovery; display names do not identify accounts.

If first setup expires or is interrupted before a passkey is enrolled, stop the
service and run as its filesystem owner:

```sh
fabric-server renew-bootstrap /etc/fabrico11y/server.conf
```

This command obtains the stopped-service lock and renews only an unfinished,
empty first-owner installation. It does not erase an established owner or
repair a corrupt access store. Read the replacement protected secret locally.

If the server refuses to open access state with a **re-bootstrap** message, the
state was written by a development build before the three-role format and is not
migrated. Stop the service, remove `state_dir/access` and start again to receive a
new bootstrap secret. This discards that installation's owner, reader tokens and
audit; Control and telemetry state are separate and unaffected.

## Passkeys and sessions

In console Settings, reverify with a passkey before changing credentials. Passkey,
session and reader-token changes require verification within five minutes. Add a
second independent passkey while an existing key is available; the server refuses to
revoke the last passkey. Passkey revocation ends all sessions. Logout and
reauthentication retire the prior browser session; separate device sessions remain
until explicitly revoked, expired or ended by a passkey change. Sessions last at most
eight hours, expire after thirty minutes idle and do not survive server restart.

## Reader tokens and read-only clients

A freshly verified owner issues a reader token from Settings
(`POST /v1/console/readers`, JSON `name` and optional `ttl_s`, default 86,400 and at
most 30 days). The token is returned once and only its hash is stored. A reader can
query telemetry (`POST /v1/console/query`) and read status and Spindle inventory
(`GET /v1/console/status`, `GET /v1/console/nodes`), and nothing else; it sees all
telemetry. List tokens with `GET /v1/console/readers` and revoke one with
`POST /v1/console/readers/ID/revoke`. To rotate, issue a new token, replace the
client's file, then revoke the old one. Revocation and expiry are immediate for
subsequent checks. Store a token in an owner-only regular file with mode 0600,
outside the client config. Do not use a symlink. Copy the
[access client example](../packaging/etc/access.conf.example) and set its exact
HTTPS origin, absolute CA path and absolute `workload_token_file`; the key keeps its
earlier name.

```sh
fabricctl access /home/operator/.config/fabrico11y/access.conf node list
fabricctl access /home/operator/.config/fabrico11y/access.conf query '<QUERY_JSON>'
```

Query JSON follows the [query contract](architecture/query.md). The API caps requests at
1,000 rows and a 24-hour window. The client sends `x-fabric-client-version: 1`, refuses
redirects and reports HTTP denial without switching credentials or endpoints. Spindle
enrollment, configuration and control are owner-only: use the console. A reader
token that calls them receives `403 role not permitted`.

## Offline owner recovery

Stop the server and identify the owner's immutable principal ID. As the service
filesystem owner, run:

```sh
fabric-server recover-access /etc/fabrico11y/server.conf OWNER_PRINCIPAL_ID
```

The command refuses an active service lock, validates durable Control state,
and saves bounded protected copies of Control and access files into one of two
`state_dir/owner-recovery-backup-N` slots. If both slots are occupied, export a
prior backup to controlled storage before freeing a slot; the command does not
overwrite it automatically. Backups contain protected identity material and may
contain an old bootstrap secret.

Recovery preserves the principal ID, rotates the installation epoch, invalidates
all reader tokens and sessions, and removes the owner's old passkeys. It issues a
ten-minute replacement secret in `state_dir/access/access-bootstrap.secret`; enroll
the owner through **First owner setup** and then add a second key. Issue new reader
tokens afterwards.

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
console release acceptance. To migrate, stop `serve-legacy`, switch to normal
`serve`, bootstrap the owner as above and issue reader tokens before using
production clients.
