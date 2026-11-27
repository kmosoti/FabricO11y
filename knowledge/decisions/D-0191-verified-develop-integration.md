# D-0191 · Verified develop integration and human release promotion

Integrate ordinary issues on `develop` in current-base, bottom-up Jujutsu PRs.
A same-repository `develop` PR and the human `main-release` gate control
promotion. Protected paths retain owner review on main. CI cache writes and trusted
image publishing use integrated `develop` only.

The branch audit found 105 main-only and 93 develop-only commits by ancestry
at `5b3638fc` and `98de7015`. Patch-ID comparison found twelve main-only
patches and no develop-only patches. Main's `03d7a958` and the audited
`develop` had the same tree. Reconciliation preserves the main product,
security, client, and packaging changes together with develop's integration
contracts. Tree equality between those earlier revisions does not establish
ancestry. Disjoint protected histories require an explicitly reviewed join if
they prevent an exact promotion.

Approvals bind the promotion PR's main base, develop head and candidate tree.
A rebase can produce a different final SHA. Qualification therefore binds the
actual protected main SHA and the concrete artifacts. Version tags reference
the qualification run and manifest that produced the published bytes.

See the [release procedure](../../docs/release-promotion.md).

## Integration evidence validator

The trusted controller validates independently collected live PR, preview, job
checkout and review facts with the versioned, read-only `xtask integration`
predicate. Its file inventory is a conservative baseline, supplemented by
semantic review of the full diff. Authenticity, independent context and
serialized landing remain controller responsibilities; supplied facts alone
do not prove them. See the [integration procedure](../../CONTRIBUTING.md#current-base-integration-evidence).
