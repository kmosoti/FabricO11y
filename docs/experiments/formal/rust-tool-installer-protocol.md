# Rust CI tool installer equivalence protocol

Registered before the Rust replacement or workflow adoption. This is a
developer-tool migration, not product verification, and it authorizes no real
Cargo installation. Acceptance and release workloads are outside scope.

## Purpose and boundary

Replace the Python launcher `tools/ci/install-cargo-tool.py` with an `xtask`
command while preserving the CI operation and its evidence. Cargo remains the
installer. The Rust command validates the explicit package/version pin,
rechecks the actual cgroup limit, selects owned DATA scratch, enforces disk
headroom and a finite deadline, captures the exact Cargo invocation and output,
cleans its temporary target, and writes the existing per-tool JSON receipt.

Keep the outer `tools/resource_group.py` containment around every invocation.
The replacement must not install into the repository checkout or RAM-backed
temporary storage. It must retain the prepared `CARGO_HOME/bin` destination and
PATH behavior so later CI steps can find the installed tool.

## Existing interface and observable records

The existing callers pass a crate and version, for example
`cargo-mutants 27.1.0`. Preserve all current pins. Use the existing
`xtask::preflight::require_limits` check: cgroup v2, finite memory maximum no
greater than 20 GiB, and zero swap. Require the configured DATA storage and
scratch roots; require scratch to be inside the admitted storage root. Reject
invalid names and empty/non-numeric version components before launching Cargo.

Invoke Cargo with the same arguments:

```text
cargo install <crate> --version <version> --locked --target-dir <owned-scratch>
```

Use a fixed 1500-second command deadline. Timeout handling must stop the
installation process tree; killing only a parent while leaving compiler
descendants would violate cleanup. Keep the timeout fixed at the public CLI.
Tests may call a private helper with a shorter finite deadline.

Keep the existing output paths:

```text
$FABRIC_STORAGE_ROOT/results/ci-tool-install/<crate>.log
$FABRIC_STORAGE_ROOT/results/ci-tool-install/<crate>.json
```

The log contains combined stdout and stderr and survives failure. The JSON
receipt retains `crate`, `version`, `target`, `exit`, `command`, optional
`error`, `cleanup_absent`, and `disk_after` (`total`, `used`, `free`). A failed
Cargo invocation or timeout returns nonzero while still recording cleanup and
the failure. Never report success if the receipt cannot be completed.

## Synthetic equivalence controls

Use a temporary fake `cargo` executable placed first in `PATH`. It records its
argv, emits separate stdout and stderr markers, and returns a configured exit
code. Do not access the network or run a real package install.

Required controls:

1. **Success:** admit a valid pinned package; verify the complete argv, the
   owned target path, combined log, exact receipt fields and DATA tool-home
   behavior; then verify the target is absent and `cleanup_absent` is true.
2. **Invalid input:** reject empty, malformed and path-like crate/version values
   before invoking the fake Cargo executable. The migrated implementation may
   tighten the old Python check for empty versions.
3. **Containment refusal:** absent scratch/storage variables, scratch outside
   storage, inadequate disk headroom, and unavailable or over-limit cgroup
   state each fail before Cargo starts.
4. **Cargo failure:** a configured nonzero exit remains nonzero; the output log
   and JSON receipt remain; the receipt records the actual exit and error; the
   scratch target is removed.
5. **Timeout:** use the private short-deadline helper and a fake Cargo process
   that owns a child. Confirm both are gone, timeout is reported as failure,
   the log is retained, the receipt is written, and scratch is removed.
6. **Receipt failure:** inject an unwritable receipt destination and confirm the
   command cannot return success. Preserve useful install failure output.
7. **Workflow wiring:** verify each required workflow call retains its exact
   package/version pin and outer resource launcher; confirm the artifact
   upload still includes `results/ci-tool-install/`.

These tests establish behavior against the registered invocation/evidence
contract. They do not prove that a package is trustworthy, that Cargo's
registry is available, or that a real install succeeds.

## Adoption boundary

Implement the Rust command and synthetic tests first. Replace workflow callers
and delete the Python launcher in a separate commit that states why required CI
gates and their receipt artifacts are preserved. Do not alter check profiles,
required gate membership, package versions, resource limits, or verifier
policy. No real install, qualification, release, or performance comparison is
part of this protocol.
