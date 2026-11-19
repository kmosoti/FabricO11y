# Fedora RPM (alpha.2)

This package targets x86_64 Fedora 44, built with Rust 1.100.0 and the checked-in lockfile. Fedora 45 requires a package signature trusted by the installing system. Before publishing, a maintainer must sign the RPM with an authorized release key and verify it; no release key is supplied here. An unsigned local test RPM is not a release artifact. No package script starts, enables, stops or migrates services or changes user data.

## Build from a source checkout

Install `rpm-build`, Rust 1.100.0, GCC, `pkgconf-pkg-config`, and Python 3. The build fails until every dependency notice is available and validated by `packaging/dependency-notices.py`. The selected locked graph builds bundled SQLite and Zstandard with a C compiler. The spec defaults its build target to `/data/cargo-target`; set `CARGO_BUILD_TARGET_DIR` to a writable disk-backed directory if that path is unavailable. Set `CANDIDATE_COMMIT` to the exact committed revision to package. Populate Cargo's cache in advance. The build is offline and fails if required crates are missing. Use disk-backed paths with sufficient space; this example does not require `/data` on an installed system.

```sh
export TMPDIR=/path/on/disk/tmp CARGO_BUILD_TARGET_DIR=/path/on/disk/cargo-target
mkdir -p "$TMPDIR" "$CARGO_BUILD_TARGET_DIR" "$HOME/rpmbuild"/{SOURCES,RPMS,SRPMS,BUILD,BUILDROOT,SPECS}
git archive --format=tar --prefix=fabrico11y-alpha.2/ "$CANDIDATE_COMMIT" | gzip > "$HOME/rpmbuild/SOURCES/fabrico11y-alpha.2.tar.gz"
rpmbuild -ba packaging/rpm/fabrico11y.spec
```

Build from the exact committed candidate, not a dirty checkout. If Rust is provided by rustup instead of RPM packages, RPM cannot verify its build dependencies; after checking them separately, pass `--nodeps` to `rpmbuild`. Record its revision, Fedora version, Rust version, RPM hashes and signature verification along with test results. `--offline` does not vendor dependencies into the source RPM.

## Configure a fresh rootless instance

Installation places executables under `/usr/bin` and examples under `/usr/share/doc/fabrico11y/examples`. It does not create configuration or storage. Choose private, writable, disk-backed directories outside `/tmp` and `/dev/shm`. The examples are templates: their `/data` and UID 1000 paths must be changed. The service environment file must provide absolute paths for `FABRIC_DATA_DIR` and `TMPDIR`; create both directories and restrict their permissions. Configure `%h/.config/fabrico11y-rpm/spindle.json` and `server.json` with absolute paths, credentials and a matching origin/authority before enabling the complete stack. See `/usr/share/doc/fabrico11y/examples/config/` and `docs/collection.md` in the source distribution for the service configuration and forwarding contract. Use `%t/spindle/spindle.sock`'s expanded path (`$XDG_RUNTIME_DIR/spindle/spindle.sock`) in the Spindle JSON.

```sh
umask 077
mkdir -p "$HOME/.config/fabrico11y-rpm" "$HOME/.local/state/fabrico11y-alpha2" "$HOME/.cache/fabrico11y-alpha2-scratch"
cat > "$HOME/.config/fabrico11y-rpm/environment" <<EOF_ENV
FABRIC_DATA_DIR="$HOME/.local/state/fabrico11y-alpha2"
TMPDIR="$HOME/.cache/fabrico11y-alpha2-scratch"
EOF_ENV
# Write spindle.json and server.json as described above, then:
systemctl --user daemon-reload
systemctl --user enable --now fabrico11y-server.service fabrico11y-spindle.service fabrico11y-daemon.service
```

The receiver starts only when explicitly requested; the daemon requires Spindle. The units use a private runtime socket, restrictive umask, and `fabrico11y.slice`; the slice is activated by the services and is not enabled separately. They intentionally use separate names and configuration from the legacy installer. Check `systemctl --user status` and the journal for each unit. Do not assume that starting a unit proves forwarding or query readiness; verify receipts and queries as described in `docs/deployment-verification.md` in the source tree.

## Alpha.1 cutover and rollback

There is no in-place store migration. Before changing anything, record the installed alpha.1 package version and package source (the published RPM is `0.1.0-0.alpha.1`), the exact executable paths and versions, active/enabled status and unit definitions (`systemctl --user cat`), configuration, credentials, and data/scratch paths. Legacy source-installed stacks commonly use `fabric-server.service`, `spindle.service`, and `fabric-daemon.service` with executables in `~/.local/bin`; inspect the actual installation rather than assuming those paths. Stop the legacy services cleanly, then make a permission-preserving backup of the original configuration, credentials, binaries or original package, and **all** custody, canonical and projection stores while stopped. Verify that the backup is readable and retained separately. These stores can contain sensitive observations and credentials.

Install the signed alpha.2 RPM using the Fedora package manager. Configure it with fresh, separate storage and credentials as above; never point it at alpha.1 stores. Ensure the old units are stopped before starting the new ones because their socket and listener may conflict. Verify collection, forwarding, and query behavior before relying on the new instance.

To roll back, stop and disable the three `fabrico11y-*` services, restore the backed-up alpha.1 package or binaries, configuration, credentials and complete stores to their original paths, reload the user manager, and start the previously enabled legacy services. Confirm their status and query the restored data. Keep the alpha.2 store separately; it cannot be merged into alpha.1 by this procedure. If alpha.1 was installed from an RPM, retain the exact old RPM and use the package manager's version-specific downgrade after stopping alpha.2; do not assume a repository still contains it. Test rollback on a copy before production use.
