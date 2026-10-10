# Release package builders

Both packages require the pinned Rust 1.99.0 toolchain and a successful console
build receipt. Run builds through `tools/resource_group.py`; its mounted data
storage, bounded cgroup and scratch paths are mandatory. These builders construct
artifacts. They do not attest installation or release acceptance.

```sh
python3 -B tools/resource_group.py -- python3 -B tools/ui/build.py --out /run/media/kmosoti/data/FabricO11y/results/console-candidate
python3 -B tools/resource_group.py -- packaging/build-deb.sh /run/media/kmosoti/data/FabricO11y/results/packages /run/media/kmosoti/data/FabricO11y/results/console-candidate
python3 -B tools/resource_group.py -- packaging/build-rpm.sh /run/media/kmosoti/data/FabricO11y/results/packages /run/media/kmosoti/data/FabricO11y/results/console-candidate
```

Create the package output directory first. Debian construction needs `dpkg-deb`,
`dpkg-shlibdeps` and `objdump`; Fedora construction needs `rpmbuild` and `objdump`.
Both builders use the same binaries, static systemd units, configuration examples,
license/notice and complete hash-verified console in
`/usr/share/fabrico11y/console`. `asset-manifest.json` records the asset identity;
`console-headers.json` preserves the build's CSP including inline script hashes.
The server reads the configured console directory and serves the shell at
`/console/` on the API's trusted HTTPS origin. Configure `access_origin` and
`access_rp_id` together before enabling `console_dir`.

Examples retain 100 GB decimal of sealed telemetry with 24-hour age expiry;
Expiry removes only whole Segments. The independent 20 GiB journal ceiling,
working scratch and Spool space require additional disk headroom. Units bound the
server with its companion below 4 GB decimal and disable swap; the shared slice
also contains independently enabled node services.

Packages leave services disabled until an operator installs configuration and
credentials. Debian remove preserves configuration/state; purge deletes both.
RPM erase and upgrade preserve configuration/state; the operator must deliberately
remove retained state separately. RPM installs run the same service-account
collision check as Debian and apply existing SELinux file contexts with
`restorecon` where available. This is no claim of Fedora SELinux service acceptance;
actual enforcing-mode start, delivery, recovery, upgrade and reboot remain required
candidate cells. No permissive mode or broad policy exception is installed.

The Apache-2.0 license and project notice are included in both packages. The locked
whole-workspace dependency graph (including WASM) supplies bundled license/notice
texts and `THIRD-PARTY-NOTICES.json` under package documentation. Construction
fails when required license material or source identity is missing or changed.
MPL-covered sources are included as unchanged locked registry archives under
`licenses/source`, with checksums and local paths in the notices. A small number
of upstream archives omit standalone texts; the manifest labels their preserved
SPDX declarations, standard terms or repository sibling license provenance and
includes the exact source with all actual notices. No copyright notice is invented.
Reviewing these recorded provenance limits remains part of release review.

To construct both formats with identical native/UI payloads, an exact successful
isolated alpha2 Debian build can supply the RPM data:

```sh
python3 -B tools/resource_group.py -- packaging/build-rpm.sh OUT CANDIDATE_DEB --candidate-deb BUILD_RECEIPT
```

The adapter rejects a wrong version or package/source identity, copies every
regular data member, and records `CANDIDATE-PROVENANCE.json` alongside the notices.
RPM uses its own metadata, dependencies, lifecycle scripts and SELinux restoration.
This preserves Debian12-sysroot binaries; it does not claim a separate Fedora
compiler result. The historical alpha1 migration fixture uses a distinct
`--historical-deb` mode and its registered hash.

## Fedora account ordering

RPM's native sysusers metadata can create accounts before `%pre` runs. This
package keeps its installed sysusers file but excludes it from that metadata;
the explicit `%pre` collision guard runs before unpacking, and `%post` then
creates the service identity. The builder rejects any resulting RPM with a
nonempty sysusers header. Native library dependency generation remains enabled.
This follows the account-ordering counterexample retained from the Fedora44
`release-fedora44-upgrade-02` trial, which failed before normal service setup.

For registered measurements, `fabric-server serve CONFIG --timing-events`
(or the explicit migration `serve-legacy` mode) forwards the opt-in flag to
its dedicated Spindle and inherits that child's stdout. The child reports
bounded source/commit/send/answer events with process, node, generation and
sequence identities. This stdout is separate from the operational file the
Spindle collects; missing clock samples and dropped events remain explicit.
Normal service commands leave timing disabled and child stdout suppressed.
