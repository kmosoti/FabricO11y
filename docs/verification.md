# Verification

Fast: formatting, compilation, tests and repository integration validation.
Medium: fast plus Clippy, separate doctests and Cargo dependency-tree inspection.
Extensive: medium plus disposable Jujutsu preservation/exclusion/rewrite controls.
No whole-product, Fedora-kernel, security-audit or formal-verification claim follows.
Dependency-tree inspection is not a vulnerability/license audit. Those integrations
and controlled model skill evaluations remain explicitly pending.

Candidate capture records exact commit ID, compiler, jj version and source-change
status. Verification refuses stale captures, materializes the exact commit into an
owned workspace, checks its diff against the parent, runs medium checks with
separate build output, rechecks source equality and writes a receipt next to the
capture. Missing tools and nonzero exits are failures, not fabricated evidence.
Source is rechecked but not an OS sandbox; untrusted execution needs separate isolation.
