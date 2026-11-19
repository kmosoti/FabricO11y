# CI tool image qualification

The `CI tool images` workflow builds common and native Fedora images from trusted
`develop` revisions that change the image or a tool pin. Rerun a failed
publisher job at its original revision. Dispatch on `develop` is available
when `main` contains the workflow. The default `standard` dispatch builds common
and native. Select `common-seeded` to build only the compiler-seed variant, or
`all` to build all three using the shared common build stage. Ordinary source
and lockfile changes leave the publisher idle. The workflow verifies the
selected images in network-disabled containers before publishing
commit-tagged images to GitHub Container Registry. The run summary records
each immutable digest.

The common image contains Rust 1.100.0, Jujutsu, Weaver, and native build
prerequisites. The native image adds Verus and its Z3, Charon and Aeneas,
Lean and its compiled libraries, cargo-fuzz, and Greptime. Image builds reuse
the repository's pinned bootstrap scripts. The native image discards upstream
archives, Cargo downloads, and intermediate native compiler outputs after
building. The image downloads precompiled Lean dependencies from the bundled
Aeneas libraries' import closure. Lake still builds any missing dependencies.
Compiled Lean libraries and the paths required by the checkers remain installed.

## Validate the qualification workflow

Use the fixture-based simulation for routine changes to qualification. It runs
the same plan validation and report logic used by the hosted comparison with
deterministic inputs. Simulated results can verify rejection of invalid evidence
and missing cache state, as well as the timing arithmetic. Use measured data
for image transfer, cache upload and restore, compiler wall time, and image
adoption decisions.

## Compare on matched hosted VMs

The `Paired CI image qualification` workflow controls for the CPU and runner
image variation between standard `ubuntu-24.04` jobs. Each job measures the
Fedora baseline and the candidate sequentially on the same runner. Three cold
jobs and three warm jobs give each variant the same runner mix in a cache
state. Cold order is baseline/candidate, candidate/baseline, baseline/candidate.
Warm order reverses that sequence. The second lane can inherit host page and
network caches. Separate workspaces, homes, containers, and fresh Docker and containerd
stores prevent direct reuse of source, build outputs, or image layers. Both
images are pulled anonymously by digest, including for warm samples.

Use the publisher's trusted `develop` source SHA and the immutable public
GHCR digest from its run summary. Put this file in
`.github/ci-images/paired-qualification.json` on a same-repository branch named
`ci/tool-image-paired-qualification/<trial>`:

```json
{
  "revision": "<publisher develop commit SHA>",
  "profile": "native-acceptance",
  "candidate": "ghcr.io/kmosoti/fabrico11y-ci@sha256:<native image digest>",
  "mode": "smoke"
}
```

1. Use `smoke` when changing the real cache transport, archive format, or
   image startup contract. One cold
   pair seeds application and baseline proof fixture caches, and one warm pair
   restores the exact keys. Inspect both pulls, the empty-store inventories
   and the fixture's DNS and cache assertions. The fixture checks executable
   bytes, ownership, permission modes, and symlink and hardlink behavior in
   fresh containers. It exercises ownership and path mechanics at small scale.
   Keep quota and full native proof archive readback as qualification checks.
2. Use `qualification` when deciding whether to adopt a new pinned image.
   Run any profiles being adopted on separate trial branches, sequentially. The first cold pair saves one application cache per variant
   and the baseline native proof cache after both measurements. Other cold
   pairs skip both restore and save. Warm pairs use the pinned Actions cache
   action, require exact sample-one hits and run one pair at a time. Keys bind
   the profile, host architecture, role, image digest, exact source revision,
   and run attempt. The versioned prefix identifies the build flags and path
   contract of this harness.
3. Inspect each artifact's `pair.json`, `host.txt`, compiler/package records,
   and per-lane `timings.tsv`. Lanes must have identical source archive bytes
   and compiler/hardware identity within each pair. Different pairs can use
   different CPUs and image revisions. The cohort includes three complete
   pairs in each cache state or fails. The report shows baseline and candidate
   medians and ranges, and the signed candidate-minus-baseline deltas.
4. Compare measured wall from anonymous image pull through the last check,
   including the original build, native engine and 60-second fuzz workloads.
   Report daemon provision and cache save separately. The timer also breaks
   down pull and startup, immutable source download/extraction, application
   and baseline proof cache restore, tool setup, and verification. Use job
   and step logs for queue and dependency time, cache byte counts, CPU and
   Ubuntu image versions, and the action SHA readback. A candidate cold median
   must improve and the candidate warm median must be at or below the baseline
   warm maximum. Review missing caches and partial results before adoption.

Both roles independently extract the same immutable GitHub archive with its
verified commit header and preserved source mtimes. The paired runner uses
Ubuntu GNU tar and zstd for Actions cache archive I/O, and its manually
created containers use a Docker bridge. The retired separate-runner workflow used different runners and checkout
methods. Its historical runs remain available for reference. Use paired warm
evidence for an adoption path with the same source mtimes and
exact source-qualified compiled artifact keys. A first-time PR or cross-source
cache hit has a different warm condition. Cargo decides freshness from source
mtimes, so dependency source cache sharing and compiled-artifact reuse have
different correctness requirements.

Record run links, outcomes and cache sizes before cleanup. For paired runs,
list by the specific branch ref with `--key ci-image-pair-v1-<run-id>-`, then
delete only those reviewed cache IDs. A new full workflow run or attempt
creates new seed keys.

## Qualify the common compiler-seed variant

Use the `common-seeded` publisher dispatch on trusted `develop`. The separate
`common-seeded-<revision>` tag points to a common tool image with a release
compiler cache for registry dependencies. The publisher requires the seeded
launcher to accept the archived source and installed tools in its offline
preflight. Common and native tags retain their tool-only contents.

For a separate paired `verify` run, use this variant and its published digest.

```json
{
  "revision": "<publisher develop commit SHA>",
  "profile": "verify",
  "candidate": "ghcr.io/kmosoti/fabrico11y-ci@sha256:<common-seeded image digest>",
  "variant": "common-seeded",
  "mode": "qualification"
}
```

The seeded lane runs the same checks and release command. Only the release
command uses the compiler seed. The lane records the preflight and raw
sccache counters. An active cold seeded lane must register Rust cache hits.
A warm lane still restores its exact role-specific Actions application cache
and may already have fresh release outputs. Cold lanes begin with empty
application and proof caches. The seed stays part of the image and never
enters those caches. The workflow times the complete image pull and release
launcher, including its cache initialization. Report this variant separately
from tool-only results and use the same cold and warm timing gates.

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
