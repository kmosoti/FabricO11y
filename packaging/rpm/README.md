# Fedora RPM (`0.1.1-0.alpha.2`)

This package targets x86_64 Fedora 44 and 45, built with Rust 1.100.0 and the checked-in lockfile. Fedora 45 requires a package signature trusted by the installing system. Before publishing, a maintainer must sign the RPM with an authorized release key (or verify SLSA build provenance with `gh attestation verify`) and verify it; no release key is supplied here. An unsigned local test RPM is not a release artifact. No package script starts, enables, stops or migrates services or changes user data.

## Build from a source checkout

Install `rpm-build`, `systemd-rpm-macros`, Rust 1.100.0, GCC, `pkgconf-pkg-config`, and Python 3. The build verifies the dependency notices with `packaging/dependency-notices.py`. The locked crates build bundled SQLite and Zstandard with a C compiler. Set `CANDIDATE_COMMIT` to the exact committed revision and populate Cargo's cache before building. The spec uses offline Cargo and defaults its build target to `/data/cargo-target`. You can set `CARGO_BUILD_TARGET_DIR` to any writable disk-backed directory with sufficient space.

```sh
export TMPDIR=/path/on/disk/tmp CARGO_BUILD_TARGET_DIR=/path/on/disk/cargo-target
mkdir -p "$TMPDIR" "$CARGO_BUILD_TARGET_DIR" "$HOME/rpmbuild"/{SOURCES,RPMS,SRPMS,BUILD,BUILDROOT,SPECS}
git archive --format=tar --prefix=fabrico11y-0.1.1-alpha.2/ "$CANDIDATE_COMMIT" | gzip > "$HOME/rpmbuild/SOURCES/fabrico11y-0.1.1-alpha.2.tar.gz"
rpmbuild -ba --define "_pkg_ver 0.1.1" --define "_alpha_tag alpha.2" --define "_alpha_rel 0.alpha.2" packaging/rpm/fabrico11y.spec
```

Build from the exact committed candidate, not a dirty checkout. If Rust is provided by rustup instead of RPM packages, RPM cannot verify its build dependencies; after checking them separately, pass `--nodeps` to `rpmbuild`. Record its revision, Fedora version, Rust version, RPM hashes and signature verification along with test results. `--offline` does not vendor dependencies into the source RPM.

## Configure a fresh rootless instance

Installation places executables under `/usr/bin` and examples under `/usr/share/doc/fabrico11y/examples`. Choose private, writable, disk-backed directories outside `/tmp` and `/dev/shm`. Adapt the example paths under `/data` and replace UID 1000 with your UID. Set absolute paths for `FABRIC_DATA_DIR` and `TMPDIR` in the service environment file below. Copy the standalone `spindle.toml` template from `/usr/share/doc/fabrico11y/examples/config/`, set its persistent store, and set its socket to the absolute expansion of `$XDG_RUNTIME_DIR/spindle/spindle.sock` for your user.

```sh
umask 077
mkdir -p "$HOME/.config/fabrico11y-rpm" "$HOME/.local/state/fabrico11y-alpha2" "$HOME/.cache/fabrico11y-alpha2-scratch"
cat > "$HOME/.config/fabrico11y-rpm/environment" <<EOF_ENV
FABRIC_DATA_DIR="$HOME/.local/state/fabrico11y-alpha2"
TMPDIR="$HOME/.cache/fabrico11y-alpha2-scratch"
EOF_ENV
# Adapt the Spindle template and save it as ~/.config/fabrico11y-rpm/spindle.toml.
systemctl --user daemon-reload
systemctl --user enable --now fabrico11y-spindle.service fabrico11y-daemon.service
```

To add canonical custody, configure Spindle forwarding using the fleet example. For a co-located receiver, adapt the `server.toml` template to `~/.config/fabrico11y-rpm/server.toml`, supply private credential files and persistent storage, and start it with `systemctl --user enable --now fabrico11y-server.service`. See [collection.md](../../docs/collection.md) for matching the origin and authority. The user services activate `fabrico11y.slice` and Spindle creates a private runtime socket. Check `systemctl --user status` and the journal for each enabled unit, then verify receipts and queries using [deployment-verification.md](../../docs/deployment-verification.md). Both guides are installed under `/usr/share/doc/fabrico11y/`.

## Alpha.1 cutover and rollback

Before changing anything, record the installed alpha.1 package version and source (the published RPM is `0.1.0-0.alpha.1`), the exact executable paths and versions, enabled and active services, configuration, credentials, and data paths. The published alpha.1 RPM installed `fabrico11y-node.service` and `fabrico11y-server.service` in the system manager. Inspect them with `systemctl cat` and cleanly stop the installed units with the system manager before upgrading. A source-installed stack can instead use `fabric-server.service`, `spindle.service`, and `fabric-daemon.service` in the user manager with executables in `~/.local/bin`. After stopping the active legacy units, make a permission-preserving backup of the original configuration, credentials, package or binaries, and all custody, canonical and projection stores. Verify the backup and keep it separately from both installations.

Install the signed alpha.2 RPM using the Fedora package manager. Configure it with fresh, separate storage and credentials as above; never point it at alpha.1 stores. Ensure the old units are stopped before starting the new ones because their socket and listener may conflict. Verify collection, forwarding, and query behavior before relying on the new instance.

To roll back, stop and disable the alpha.2 user services. If alpha.1 was installed from an RPM, downgrade using the exact retained RPM. Restore the legacy binaries or package, configuration, credentials and complete stores to their original paths. Reload the manager that owns the legacy units and restore their recorded enabled and active states. Confirm their status and query the restored data. Keep the alpha.2 stores separately for recovery.
