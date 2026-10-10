# Local Debian VM installation acceptance path

This is an isolated local measurement route for the registered installation
acceptance script. It does not replace or amend the registered protocol's
disposable-container environment and is not deployment qualification. The VM
uses Debian 13 with systemd as PID 1 and a unified cgroup v2 hierarchy; the
existing A1–A13 checks and negative controls remain the assertions.

## Prepare and run

Build the package from the private, source-hashed snapshot prepared for this
run. The package build needs Rust 1.98.0, `dpkg-deb`, `dpkg-shlibdeps`, and
`objdump`; the host lacks the two dpkg build tools, so perform the build in the
Debian 13 build VM/container used for the frozen snapshot, then copy the
resulting `.deb` to the mounted data drive. Do not build from a moving main
worktree. Record the snapshot commit and the package SHA-256.

Run the acceptance VM from the repository root:

```sh
python3 tools/resource_group.py -- python3 tools/qualification/install/run-qemu.py \
  --deb /run/media/kmosoti/data/FabricO11y/artifacts/fabrico11y_0.1.0~alpha.1_amd64.deb \
  --source-commit <40-or-64-character-snapshot-commit> \
  --run-id install-debian13-local-01
```

The runner verifies the outer cgroup v2 resource group (20 GiB memory maximum,
no swap), checks the official Debian cloud-image SHA-512 manifest, and caches
the verified base image under `cache/installation-qemu` on the data drive. It
creates an 8 GiB virtual-size copy-on-write guest disk and boots QEMU TCG with
6 GiB RAM and two vCPUs. It uses an ephemeral SSH key, localhost-only port
forwarding and a disposable Debian guest; no host package installation,
privileged command, KVM access or system service change is needed. Boot is
bounded at 10 minutes, acceptance at 15 minutes, and the resource-group
deadline is 30 minutes.

The VM invokes the same `acceptance.sh` with the additive
`--self-spindle-ca /etc/fabrico11y/ca.pem` argument. This configures the
server's dedicated local Spindle companion to trust the acceptance CA; it does
not change the A1–A13 expectations. The existing `run.sh` invocation remains
compatible without the optional argument.

Logs and `receipt.json` are written to
`results/installation-qemu/<run-id>` on the data drive. Temporary image
overlays, seed ISO, SSH keys and the guest process are cleaned up on exit. If
the guest cannot be stopped, scratch is retained for inspection and the
receipt records that cleanup did not complete. Preserve that failure evidence
before manual cleanup. Record a successful run as a local VM measurement, not
as the registered container result or deployment qualification.

## Current host prerequisites

The inspected Fedora host has QEMU TCG, `qemu-img`, `genisoimage`, OpenSSH,
curl and cgroup v2. `/dev/kvm` is not available to the user; the runner does
not require it. `dpkg-deb` and `dpkg-shlibdeps` are unavailable on the host, so
package assembly must occur in the Debian build environment. Existing release
binaries inspected on the host require at most GLIBC 2.34, matching the
packaging limit; no compatibility claim is made for a package that has not yet
been built and inspected.
