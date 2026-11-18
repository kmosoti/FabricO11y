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

## Qualify the candidate

The `CI image qualification` workflow compares baseline and candidate runs
for one job profile. The baseline uses the Fedora image and installers from
Development checks. The candidate uses an immutable tool image and checks the
installed tool identities. Every sample uses a fresh `ubuntu-24.04` x86-64
runner with the same source SHA, compiler settings, checks, and 60-second
fuzz budget used by a `develop` push. The workflow
preserves the native engine binary checksum and version checks.

1. Wait for the publisher on `develop` and record its source SHA and both
   digests from the run summary. Confirm the GHCR package is publicly readable
   so a fork PR can start a required job without registry credentials.
2. On a same-repository branch named `ci/tool-image-qualification/<trial>`
   containing these workflow files, add `.github/ci-images/qualification.json`
   and push. Use `verify` with the common digest and `native-acceptance` with
   the native digest. The plan requires the source SHA to be on `develop`
   and in the public image's OCI source label. The digest must match the
   published `common-<revision>` or `native-<revision>` tag for the profile.
   The measured revision must have the same Development checks workflow and
   cache actions as the trial branch. Use `"samples": 1` for a pilot and `"samples": 3` for a
   qualification cohort. Omitting the field starts a pilot.

   ```json
   {
     "revision": "<publisher develop commit SHA>",
     "profile": "verify",
     "candidate": "ghcr.io/kmosoti/fabrico11y-ci@sha256:<common image digest>",
     "samples": 3
   }
   ```

3. Start with a pilot if runner capacity is uncertain. The plan logs Docker
   storage on its runner. Each trial logs its own free space after container
   initialization, after cache setup, and at job end. Retain container
   initialization failures as qualification evidence. Run one complete
   qualification cohort per profile, sequentially and with cache reclamation
   between them. Use the same source SHA and corresponding candidate digests.
4. Cold runs skip cache restore. The first cold job in each variant saves
   its application cache and, for baseline native, its proof-tool cache.
   The two other cold jobs in the cohort also skip cache save. Three fresh
   warm jobs in each variant read its same immutable seed caches, with one
   warm reader per variant at a time. A missing or evicted cache invalidates
   the warm run. Cache seeds share repository storage with development
   caches. Capture their sizes promptly with `gh cache list --ref
   refs/heads/ci/tool-image-qualification/<trial>`. A full run rerun uses new
   cache keys and artifacts. Rerun all jobs after fixing any cohort failure.
5. Capture job and step timestamps with
   `gh api --paginate repos/kmosoti/FabricO11y/actions/runs/<run-id>/attempts/<attempt>/jobs`.
   Report queue and dependency wait separately from job execution. Record
   container initialization, cache restore and save, tool setup, compilation
   and every verification phase. Fingerprints must match exactly for CPU
   model, usable cores, CPU quota, cgroup memory limit, runner kernel, product
   and native compiler versions, and native library versions. The jobs also
   retain raw physical memory from `MemTotal`. Across the matched identities,
   the difference between the highest and lowest value must be at most
   1,024 KiB.
6. Read each job's `Set up job` log with
   `gh run view --job <job-id> --log`. Record the version under `Runner Image`
   for Ubuntu and the resolved checkout and cache action SHAs from the action
   download lines. Cold controls intentionally skip cache actions. Group
   results by the exact revisions and report matched baseline and candidate
   counts for each job profile and cache state. Compare medians from at least three successful
   samples per variant in a matching group. Keep logs and timings from
   earlier or mixed groups with their original labels and list their
   unmatched counts separately. Compare wall times through the end of
   verification. Report cache priming and full job times separately, along
   with image and cache sizes and the median and range for each cell.
7. Confirm that the candidate preflight and checks passed with the installed
   tools. Adopt only if each candidate cold median improves and each candidate
   warm median falls within or below the observed baseline warm range. Retain
   run links and report fails, cache evictions, and material runner differences
   alongside the timings.

After recording the run results and cache sizes, reclaim only its trial caches.
List by its exact branch ref and filter keys by the run ID. Review the list
and delete each matching cache by ID with `gh cache delete <cache-id>`.

```sh
gh cache list --ref "refs/heads/ci/tool-image-qualification/<trial>" \
  --key "ci-image-trial-v1-<run-id>-" --json id,key,ref,sizeInBytes
```

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

Use the source SHA and published profile digest as above. Put this file in
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

1. Run `smoke` with the native image before measuring full workloads. One cold
   pair seeds application and baseline proof fixture caches, and one warm pair
   restores the exact keys. Inspect both pulls, the empty-store inventories
   and the fixture's DNS and cache assertions. The fixture checks executable
   bytes, ownership, permission modes, and symlink and hardlink behavior in
   fresh containers. It exercises ownership and path mechanics at small scale.
   Keep quota and full native proof archive readback as qualification checks.
2. Run `qualification` for each profile on separate trial branches,
   sequentially. The first cold pair saves one application cache per variant
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
created containers use a Docker bridge. The earlier separate-job cohort ran
the GitHub service's job container startup and checked out Fedora by archive
and the prebuilt image by Git. Keep the two cohorts labeled separately. Use
paired warm evidence for an adoption path with the same source mtimes and
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
