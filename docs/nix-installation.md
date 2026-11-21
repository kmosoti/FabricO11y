# Nix and NixOS installation

The FabricO11y flake builds six executables for `x86_64-linux` using Rust
1.100.0 and the repository's Cargo lockfile. Run these commands from the
release checkout or use `github:kmosoti/FabricO11y/v0.1.0-alpha.2` as the
flake reference. Alpha.2 uses the stable NixOS 26.05 package set.
`fabrico11y` and `fabrico11y-26_05` select the same package and app.

```sh
nix build .#fabrico11y --no-link
nix run .#fabrico11y -- --help
nix profile add .#fabrico11y
```

The profile exposes `fabric`, `spindle`, `fabric-server`, `fabric-adapter`,
`fabric-query`, and `fabric-mcp`. An ordinary Nix profile install places
the executables on `PATH`. Use the [collection guide](collection.md) to run
services on Linux. Packaged TOML examples are in
`share/fabrico11y/config` under the package output.

## NixOS user services

Point `inputs.fabrico11y` at the published FabricO11y tag. On a NixOS 26.05
host, add `inputs.fabrico11y.nixosModules.fabrico11y` to the host's `modules`
list. The `fabrico11y-26_05` module name is an alias.

```nix
{
  services.fabrico11y = {
    enable = true;
    user = "alice";
    dataDir = "/var/lib/fabrico11y/collector";
    scratchDir = "/var/lib/fabrico11y/scratch";
    spindleConfigFile = "/var/lib/fabrico11y/spindle.toml";
  };
}
```

Before activation, create the state and scratch directories on disk and
make them private and writable by the named regular user. Install the
TOML files outside the Nix store. The module requires normalized absolute
paths and rejects the Nix store and known volatile directories such as
`/run`, `/var/run`, and `/tmp`. Resolve any symlinks or bind mounts
in the paths on the host and verify the targets are on persistent
disk-backed storage. Keep the configuration and credential files private.

Match the Spindle `socket` to
`/run/user/<uid>/spindle/spindle.sock` for that user and set a persistent
`storage` path. Use the [standalone](../config/spindle.toml) or
[fleet](../config/spindle-fleet.toml) example as appropriate. Create the
storage parent first. Configure credentials with external private files as
described in [canonical custody](canonical-custody.md).

The module starts Spindle and the collector in the named user's manager
during their session. To run them before login, opt into
`users.users.alice.linger = true`. To run a co-located canonical receiver,
set `services.fabrico11y.serverConfigFile` to an external
[server TOML](../config/server.toml). Configure its loopback endpoint,
persistent directories and external credential files before activation.
Keep token bytes outside Nix source and outside the store.

After a rebuild, check the services from the selected user's session.

```sh
systemctl --user status spindle.service fabric-daemon.service
fabric --data-dir /var/lib/fabrico11y/collector \
  --tmp-dir /var/lib/fabrico11y/scratch production-ready \
  --spindle-socket "$XDG_RUNTIME_DIR/spindle/spindle.sock" --json
```

If you previously installed these services manually, stop them and preserve
their local unit files before enabling the module. The user's own unit
files can take precedence over NixOS-managed units. Before disabling or
rolling back the module, stop the user services. Back up the external
configuration, credentials, and state separately from the NixOS
generation. Rolling back the generation changes the package and module
configuration. Restoring telemetry state requires the matching durable
store backup.

## Release qualification

The VM check builds the package from the release candidate with the locked
stable NixOS 26.05 input and exercises the NixOS module.

```sh
nix flake check --no-build
nix build --no-link .#checks.x86_64-linux.nixos
```

## Loopback-preserving remote proxy (private IPv6 / WireGuard mesh)

`fabric-server` and Spindle forwarding intentionally require loopback endpoints
(`127.0.0.1` or `::1`). When connecting nodes across a private IPv6 or
WireGuard mesh without exposing `fabric-server` directly on non-loopback
interfaces, enable `services.fabrico11y.remoteProxy`:

- **Canonical receiver (`role = "ingress"`):** Set `serverConfigFile`,
  `remoteProxy.enable = true`, `remoteProxy.remoteEndpoint = "[2001:db8::10]:17440"`,
  and optionally `remoteProxy.interface = "wg0"` with `openFirewall = true`.
  `systemd.sockets.fabrico11y-remote-proxy` binds the mesh socket (with
  `FreeBind = true`) and `systemd-socket-proxyd` forwards to unbracketed
  `::1:17440` (required because `systemd-socket-proxyd` splits `HOST:PORT` on
  the final colon without stripping brackets).
- **Forwarding node (`role = "egress"`):** Keep `serverConfigFile = null`, set
  `remoteProxy.enable = true`, and point `remoteProxy.remoteEndpoint` at the
  receiver's mesh address. Spindle forwards to `[::1]:17440`, and
  `systemd-socket-proxyd` forwards unbracketed across the mesh.
