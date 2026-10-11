# Implementation handoff

Read `SECURITY-CHANGE-SPEC.md` first. It is the implementation contract; it is not a report of completed repairs.

## Inputs

The reviewed repository is `kmosoti/FabricO11y`, base commit `18f6b367d0f34886847ea25b9168e50eb0425300`. `change-manifest.json` pins 12 original source blobs and 24 source-level operations. `original-source-edit-map.md` is the readable map. `acceptance-tests.json` and `acceptance-tests.md` define 53 required acceptance cases and their negative controls.

Start with this **read-only** check, using the actual checkout path:

```sh
python3 /path/to/this-bundle/verify_base.py /path/to/FabricO11y
```

Exit 0 means the reviewed files and original anchors match. Exit 3 means current files require re-anchoring and review; do not overwrite them. Exit 2 means the baseline or manifest is unavailable/invalid. This tool does not fetch, build, scan, execute repository code, or change the checkout. It was self-tested on a synthetic repository, not run against a FabricO11y checkout in this chat.

## Work order

Use a capability branch such as `milestone/security-hardening`, respecting existing local work and the repository's `AGENTS.md`. Read current contracts and affected tests. Record baseline identities, threat assumptions, workload limits and check expectations before implementation.

Implement R0 through R5 in the main spec. Restore trustworthy verification first. Keep policy/checker changes in separate commits. For each candidate, produce a counterexample, implement the minimum surviving mechanism, verify through independent observations, then accept or reject with an explicit record. Never adjust an oracle to make a candidate pass. Do not count a compiler error or unavailable environment as a successful semantic negative control.

Use the repository's existing resource launcher for every actual project build, test, experiment and validator. Do not substitute this bundle's synthetic probes for repository verification. Preserve failures and stop only owned processes/cgroups/worktrees when rejecting a candidate. Preserve the user's uncommitted work and published history.

## Non-negotiable limitations

Admission is not authorization. Per-peer limits do not solve distributed attacks or separate users sharing one peer. OTLP deadlines do not isolate continuously reconnecting hostile local users. Symlink rejection does not authenticate file provenance or protect against an OS owner. A proposed CI repair is not a successful verification run.

The spec does not authorize production load, privileged production installation, release/tag creation, workspace-permission changes, or a weakened security contract. No GitHub writes, cloud scans, or application tests were performed to create the bundle.

## Completion record

Return the actual implementation commit; edit-to-test traceability; exact check commands/exits and source/checker identities; positive and negative-control observations; measured resources; retained/rejected candidates; cleanup outcome; and remaining risks. Report `not_run`, `blocked`, `inconclusive`, `mitigated`, and `verified` precisely.
