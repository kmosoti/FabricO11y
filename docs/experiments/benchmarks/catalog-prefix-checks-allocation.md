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
