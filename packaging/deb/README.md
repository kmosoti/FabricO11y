# Debian 12/13 package and portable Linux tarball

The native x86_64 Debian package and portable Linux tarball are built on Debian 12 (`bookworm`, `glibc 2.36`). The `v0.2.0-rc.1` package preflight uses `fabrico11y_0.2.0~rc.1_amd64.deb` and `fabrico11y-0.2.0-rc.1-x86_64-unknown-linux-gnu.tar.zst`. The Debian version is `0.2.0~rc.1`, which sorts before `0.2.0`.

Building directly against Debian 12 `glibc 2.36` keeps every binary (`fabric`, `spindle`, `fabric-server`, `fabric-adapter`, `fabric-query`, `fabric-mcp`) compatible with **Debian 12 (`bookworm`, glibc 2.36)** and **Debian 13 (`trixie`, glibc 2.41)** without host-glibc `patchelf` bundling or copying shared `libc.so.6` / `ld-linux-x86-64.so.2` files.

## Verify downloads

Use the selected package version in the filenames below. For a published release, verify `SHA256SUMS` and the GitHub SLSA build provenance attestation before installing. Develop preflight uploads are for candidate review. Their Actions artifact digests are separate from release attestations.

```sh
sha256sum --check SHA256SUMS --ignore-missing
gh attestation verify fabrico11y_0.2.0~rc.1_amd64.deb --repo kmosoti/FabricO11y
gh attestation verify fabrico11y-0.2.0-rc.1-x86_64-unknown-linux-gnu.tar.zst --repo kmosoti/FabricO11y
```

To verify the ELF symbol floor locally (`<= GLIBC_2.36`):

```sh
packaging/deb/check-glibc.sh /usr/bin/fabric /usr/bin/spindle /usr/bin/fabric-server /usr/bin/fabric-adapter /usr/bin/fabric-query /usr/bin/fabric-mcp
```

## Install on Debian 12 or 13

```sh
sudo apt-get update
sudo apt-get install -y ./fabrico11y_0.2.0~rc.1_amd64.deb
```

For the portable tarball on other `x86_64` Linux distributions with `glibc >= 2.36`:

```sh
tar --zstd -xf fabrico11y-0.2.0-rc.1-x86_64-unknown-linux-gnu.tar.zst
```

No package script starts, enables, stops, or deletes services or telemetry storage.

## Configure a rootless instance

Create private disk-backed directories (never under `/tmp` or `/dev/shm`), install configuration templates from `/usr/share/doc/fabrico11y/examples/config/` into `~/.config/fabrico11y/`, and enable the user services:

```sh
umask 077
mkdir -p "$HOME/.config/fabrico11y" "$HOME/.local/state/fabrico11y" "$HOME/.cache/fabrico11y-scratch"
cat > "$HOME/.config/fabrico11y/environment" <<EOF_ENV
FABRIC_DATA_DIR="$HOME/.local/state/fabrico11y"
TMPDIR="$HOME/.cache/fabrico11y-scratch"
EOF_ENV
# Adapt spindle.toml (or spindle-fleet.toml) into ~/.config/fabrico11y/spindle.toml
systemctl --user daemon-reload
systemctl --user enable --now fabrico11y-spindle.service fabrico11y-daemon.service
```

For headless Debian servers running without an interactive desktop login, enable lingering for the dedicated service account:

```sh
sudo loginctl enable-linger "$USER"
```

## Private IPv6 / WireGuard mesh proxy (`systemd-socket-proxyd`)

`fabric-server` and Spindle forwarding bind strictly to loopback (`[::1]:17440` or `127.0.0.1:17440`). On Debian 12 (`systemd 252`) and Debian 13 (`systemd 257`), `systemd.socket` `ListenStream=` uses bracketed IPv6 syntax (`[2001:db8::10]:17440`), while `systemd-socket-proxyd` splits `HOST:PORT` on the last colon without stripping brackets and therefore requires **unbracketed** destinations (`::1:17440` for ingress, `2001:db8::10:17440` for egress):

- Receiver ingress service: `ExecStart=/lib/systemd/systemd-socket-proxyd --connections-max=32 ::1:17440`
- Collector egress service: `ExecStart=/lib/systemd/systemd-socket-proxyd --connections-max=32 2001:db8::10:17440`

## Build locally

```sh
export TMPDIR=/data/tmp CARGO_BUILD_TARGET_DIR=/data/cargo-target
mkdir -p "$TMPDIR" "$CARGO_BUILD_TARGET_DIR" "$TMPDIR/deb-out"
packaging/deb/build-deb.sh --out-dir "$TMPDIR/deb-out"
packaging/deb/build-tarball.sh --bin-dir "$CARGO_BUILD_TARGET_DIR/release" --out-dir "$TMPDIR/deb-out"
```
