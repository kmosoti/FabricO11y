# Release promotion

## Integrate and freeze

Develop issue changes in current-base PRs, with `verify` and
`native-acceptance` and an independent review. Land one layer at a time.
Only the owner can perform a release promotion or approve `main-release`.

Freeze integration for the promotion window. Open a same-repository PR from
`develop` to `main`. The promotion check records the PR, base SHA, head SHA,
candidate tree and manifest SHA-256. The simulated merge must match the exact
`develop` tree. After environment approval, the workflow reads both branch
tips and the current PR again. A changed head or base requires a fresh check
and approval. Before merging, the owner confirms both live tips and the
recorded candidate digest. If reviews or checks cover an earlier revision,
rerun them for the current candidate.

When a promotion uses rebase merge, record the actual `main` SHA that GitHub
produces. Its tree must equal the reviewed candidate tree. Run final checks on
that SHA even when the trees match. Inspect both histories before the next
promotion. If they diverge and the simulated merge fails or produces a different
tree, resolve ancestry through a separately reviewed reconciliation before
retrying. A clean test merge can still fail under the configured rebase
method, because rebase replays individual commits. The owner reviews the
ancestry and decides whether an explicitly authorized merge commit is
necessary. Keep the main review, required checks and deployment gate active.

## Qualify the final artifacts

1. Verify the actual `main` SHA with the required source checks and acceptance
   evidence. Keep the associated reports with the release.
2. Run **Release packages and provenance** from `main`, with the intended tag
   as its version input and select `qualify`. To verify packaging on
   `develop` or a feature branch, select `preflight`. Preflight builds and
   uploads the Fedora RPM, Debian package and glibc-baseline tarball with read
   access. A `develop` preflight also uploads a separate static browser archive
   built with the pinned release optimizer. It includes no gateway, service, or
   holder authentication and does not qualify a frontend release profile. It
   includes the project license; dependency license and advisory review remains
   necessary before publication.
   The qualification run also uploads the
   combined bundle, records its source tree and hashes, and attests the bytes.
3. After qualification succeeds, download and inspect `release-bundle`. Record
   the numeric qualification run ID and SHA-256 of `RELEASE-EVIDENCE.json`.
   Confirm the final SHA is still the protected main tip.

## Publish

The owner creates and pushes an annotated version tag whose target is the
qualified final SHA and whose message is a single JSON document:

```json
{
  "qualification_run_id": "<numeric-run-id-as-a-string>",
  "manifest_sha256": "<64-lowercase-hex-digits>"
}
```

The tag workflow verifies the qualification run, annotated tag object, source
commit, source tree, manifest and artifact hashes. Only the publication job
can create the release. It waits for the owner's approval in `main-release` and publishes the
same qualified bytes. The environment must permit the PR promotion context and
protected version tags. Keep tag creation and updates restricted to the owner.
