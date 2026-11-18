# CI tool image qualification

The `CI tool images` workflow builds common and native Fedora images from trusted
`develop` revisions that change the image or a tool pin. An explicit dispatch
on `develop` can retry a build. The workflow verifies installed tools in
network-disabled containers before publishing commit-tagged images to GitHub
Container Registry. The run summary records both immutable digests.

The common image contains Rust 1.100.0, Jujutsu, Weaver, and native build
prerequisites. The native image adds Verus and its Z3, Charon and Aeneas,
Lean and its compiled libraries, cargo-fuzz, and Greptime. Image builds reuse
the repository's pinned bootstrap scripts. The native image discards upstream
archives, Cargo downloads, and intermediate native compiler outputs after
building. Lean libraries and the paths required by the checkers remain installed.

## Qualify the candidate

1. Confirm both digests can be pulled by the required job's credential policy.
   Public repositories need anonymously readable packages for fork PRs.
2. Compare the existing workflow and the candidate digests on equivalent
   `ubuntu-24.04` x86-64 hosted runs. Use the same product revision, checks,
   compiler flags, and fuzz budget. In the candidate workflow, replace the
   container references and run `check-ci-image-tools.sh common` or `native`
   in place of the tool installation steps. Use isolated cache keys for cold
   runs and repeat the same keys for warm runs. Collect at least three samples
   for each configuration.
3. Compare queue time separately. Record container initialization and pull,
   cache transfer, tooling setup, compilation, and each verification phase.
   Record published image and cache sizes, median wall time, and variation.
4. Confirm the candidate uses the installed tools without per-job upstream
   tool downloads. Both required jobs must pass. Adopt only when median cold
   wall time improves and warm results stay within baseline variation.

## Adopt or revert

Use the validated `ghcr.io/kmosoti/fabrico11y-ci@sha256:…` digests in a separate
reviewed workflow change. Keep version checks as preflight. Remove application
job tool installs and proof-tool archives from Actions cache only once the
digests are in use. Keep the native engine version and checksum validation.
Key compiled artifacts to the image digest, compiler, architecture, build
profile and flags, and lockfiles. Cargo sources and build artifacts have
different lifecycles and can use distinct keys.

For a future tool-pin change, run the required jobs with the Fedora bootstrap
path while reviewing that change. The trusted publisher can produce the new
image digests after the pins reach `develop`. Then qualify and adopt the digests
in a follow-up. If tooling or build measurements regress, restore the previous
workflow revision. Release promotion remains governed by the main human gate.
