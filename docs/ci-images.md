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

The `CI image qualification` workflow runs one paired sample for one job
profile. The baseline uses the Fedora image and installers from Development
checks. The candidate uses an immutable tool image and checks the installed
tool identities. Each variant runs once cold and once warm on fresh
`ubuntu-24.04` x86-64 runners with the same source SHA, compiler settings,
checks, and 60-second fuzz budget used by a `develop` push. The workflow
preserves the native engine binary checksum and version checks.

1. Wait for the publisher on `develop` and record its source SHA and both
   digests from the run summary. Confirm the GHCR package is publicly readable
   so a fork PR can start a required job without registry credentials.
2. On a same-repository branch named `ci/tool-image-qualification/<trial>`
   containing these workflow files, add `.github/ci-images/qualification.json`
   and push. Use `verify` with the common digest and `native-acceptance` with
   the native digest. The plan requires the source SHA to be on `develop`
   and in the public image's OCI source label. The measured revision must
   have the same Development checks workflow and cache actions as the trial
   branch.

   ```json
   {
     "revision": "<publisher develop commit SHA>",
     "profile": "verify",
     "candidate": "ghcr.io/kmosoti/fabrico11y-ci@sha256:<common image digest>"
   }
   ```

3. Start with one pair per job and check the native image fits on the hosted
   Docker filesystem. The plan logs Docker storage on its runner, and each
   trial logs its own free space after container initialization, after cache
   restore and setup, and at job end. Retain container initialization failures
   as qualification evidence. Run at least three complete pairs per profile,
   on trial branches with the same product revision and corresponding digests.
   Start a new sample or rerun all jobs when a pair fails.
4. Cold jobs skip cache restore and save fresh, run-attempt-scoped caches.
   Each warm job requires an exact application cache hit from its cold job
   and, for baseline native, an exact proof-tool cache hit. A missing or evicted
   cache invalidates that warm sample. Run one pair at a time when repository
   cache capacity is tight. Qualification caches share that capacity with
   development caches. Capture their sizes promptly with `gh cache list --ref
   refs/heads/ci/tool-image-qualification/<trial>`.
5. Capture job and step timestamps with
   `gh api --paginate repos/kmosoti/FabricO11y/actions/runs/<run-id>/attempts/<attempt>/jobs`.
   Report queue and dependency wait separately from job execution. Record
   container initialization, cache restore and save, tool setup, compilation
   and every verification phase. The workflow rejects differences in runner
   kernel, product Rust and Cargo versions, and native compiler and library
   package versions. Check the runner image and resolved checkout and cache
   action SHAs in the logs across runs before comparing samples. Compare each job's
   cold and warm wall times through the end of verification. Report cache
   priming and full job times separately, along with published image and cache
   sizes and the median and range for each cell.
6. Confirm that the candidate preflight and checks passed with the installed
   tools. Adopt only if each candidate cold median improves and each candidate
   warm median falls within or below the observed baseline warm range. Retain
   run links and report fails, cache evictions, and material runner differences
   alongside the timings.

After recording a pair's results and cache sizes, reclaim only its trial caches.
List by its exact branch ref and filter keys by the run ID. Review the list
and delete each matching cache by ID with `gh cache delete <cache-id>`.

```sh
gh cache list --ref "refs/heads/ci/tool-image-qualification/<trial>" \
  --key "ci-image-trial-v1-<run-id>-" --json id,key,ref,sizeInBytes
```

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
