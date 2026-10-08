# Prefix-progress final check allocation

Prospective resource-only amendment to the `checks` phase of the
[prefix-progress protocol](catalog-prefix-progress-protocol.md), 2026-10-08.
Baseline and candidate commands and acceptance criteria are unchanged.

Inspection of the historical completed-fast receipt
`data/lab-completion-run-01/recovery/catalog-range-evidence-checks-01/test.json`
shows 264.130 seconds for the workspace test gate alone. The proposed 200-second
whole-profile deadline therefore lacks a defensible allowance. Correct that
before dispatch, preserving all historical outcomes.

The checks phase receives 380 seconds rather than 200. Transfer a total of 380
unused preparation seconds to verification, replacing the prior 200-second
transfer: preparation cap 3160 seconds, verification cap 4040 seconds. The
86400-second campaign, 14400-second frontier, 20 GiB maximum/no-swap containment,
1800-second outer deadline, source preservation and 192 KiB evidence reservation
remain unchanged. Admission must check actual remaining time before launch;
this is neither a fresh campaign nor an automatic retry allowance.

Run the unchanged fast and manual documentation profiles. Preserve exact exits,
receipts and any timeout. Do not loosen a test or omit a gate to meet the deadline.
The driver freezes this amendment alongside the original protocol in the new
checks receipt. Existing baseline/candidate receipts retain their original
resource and source identities.

## Revised before the checks launch

The candidate phase passed all eight sealer controls, then reached its deadline
during the history target (six tests had completed successfully). Preserve this
interrupted outcome and the launcher-owned failure tree for unit
`fabric-work-e924e62f84504e13bd441be97ddf6718`; it is not a completed history run.

Remaining frontier time is now approximately 366 seconds. Request 350 seconds
for checks, retaining the 4040-second verification and 3160-second preparation
caps above. Use the workspace's default test concurrency for the fast profile;
the initial diagnostic forced one test thread. The cgroup still contains all
descendants. No test, expected result or gate changes. The full fast profile
reruns the independent history target and sealer controls.

Before checking, confirm the failed unit is inactive, archive its entire owned
failure tree with a 512 KiB decoded bound, compare every saved byte with existing
archive controls, and remove that owned tree only after exact readback. Include
the compressed archive in the unchanged 192 KiB phase reservation. If it cannot
fit, retain the original and do not dispatch. Record preservation separately from
test results. This is prospective execution/cleanup scope, not a retroactive
extension of the interrupted candidate run.
