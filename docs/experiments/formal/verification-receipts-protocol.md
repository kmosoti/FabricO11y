# Rust receipt verification migration

Registered before candidate implementation or measurements. The owner authorized
Rust tooling and verification migration, then paused alpha acceptance until the
verification strategy is developed and optimized. This investigation does not
execute or qualify the alpha acceptance campaign.

## Contract and primitives

The historical migration exercised the Python receipt validator on explicitly
synthetic fixtures and compared it with `cargo xtask verify-receipts`. Preserve
the Python delivery, query and rate product oracles. The receipt verifier is a
structural checker, not a product oracle: after its independent expected
outcomes are recorded in
[`xtask/tests/fixtures/receipt_contract_cases.json`](../../../xtask/tests/fixtures/receipt_contract_cases.json),
the Rust CLI is the maintained runtime validator. The fixture is maintained
from the accepted contract below; it must not be generated from either
implementation. Production CI adoption remains separate from this policy
decision and requires its own implementation and workflow evidence.

The historical Python implementation used by the differential is preserved by
the immutable Git object at commit
[`f001175ff4006e2f95294458ce36f4f089d3be7d`](https://github.com/kmosoti/FabricO11y/tree/f001175ff4006e2f95294458ce36f4f089d3be7d)
(`tools/ci/verify_receipts.py`, SHA-256
`912ede91d27277bb0e45a337d1266ec1ea191b1029a536ca9f1266ced3559a23`). Historical
baseline and differential result directories remain the evidence for the
observed Python outcomes. Reproduction of the old Python-vs-Rust benchmark
requires an isolated worktree at that immutable source revision and the
historical experiment command; deleting the script from the maintained branch
must not be described as preserving an immediately runnable current-tree
benchmark. New checks use the contract fixture and the Rust CLI, not a copied
Python verifier.

The subsequent [representation policy](../../formal/verification-receipt-contract.md)
records four baseline-observed ambiguities and prospective strict rejection.

Reuse the existing bounded strict JSON decoder and descriptor-relative artifact
reader where their contracts fit. A receipt check must require the current clean
Git identity, original registry profile and commands, complete receipts, no
running marker, successful exact integer exit status, nonnegative integer
duration and matching ordinary output-log bytes. File replacement, symlinks,
nonregular files and resource exhaustion must fail closed. Do not interpret
structural receipt validation as evidence that synthetic commands actually ran.

For N selected checks, K total command arguments and B input bytes, target
O(N + K + B) validation after registry indexing, O(N + K) retained metadata and
bounded streaming buffers. Hashing every admitted log has an Omega(B) lower bound
when arbitrary byte modification must be detected without a trusted prior hash.
Input ordering and deterministic reporting may add sorting cost; disclose it.
Do not claim globally minimal verification time or security certification.

## Baseline before implementation

A Rust experiment driver may generate fixtures and invoke the unchanged Python
CLI; it must not implement the candidate decision rule. Store fixtures and raw
stdout/stderr under a newly created DATA results directory marked synthetic.
Record actual clean source, commands, exits and whole-launcher stop receipts.

Exercise a complete positive set and individual missing receipt/log, symlink
receipt/log, running marker, stale commit, dirty source flag, wrong command/ID/
profile/result/log name/version, nonzero or boolean/float exit, negative or
boolean/float duration and modified log controls. Baseline mismatch is evidence,
not permission to silently change expected behavior. The contract fixture
records the four baseline-observed legacy acceptances as deliberate Rust
refusals under the accepted representation policy. They are not exact legacy
parity and must not be counted as ordinary shared negative controls.
Nonregular and mutation-during-read controls apply to the candidate's file
primitive; do not hang the old reader on a FIFO to demonstrate a known hazard.

## Differential and performance experiments

After the baseline stops and is inspected, implement the narrow Rust slice.
Run every common control through both binaries and preserve disagreements.
Inject at least one representative checker defect and require rejection by the
independent corpus. Preserve meaningful failures and minimized regressions.

The registered historical differential benchmark compared standalone release
Rust with the Python CLI identified above, including process startup and all
identity/registry/log validation. Reproduction must use an isolated worktree at
that immutable source commit; the maintained branch no longer promises a live
Python benchmark implementation. Compilation is outside measurement but its
command and exit remain recorded. Use all extended IDs,
synthetic logs of 64 bytes, 64 KiB and 4 MiB per ID, one warmup per implementation
and corpus, and seven alternating paired repetitions. Compare decisions and
normalized inventories before reporting timing. Record wall time, CPU and peak
RSS when actually measured; do not infer one from another. Reused page cache and
host conditions are part of the reported bounds. Keep all failed trials.

Use the existing resource launcher, original enforced caps, fresh DATA reserve
and serial heavy-job scheduler. No GPU implementation, weakened checks, release
or tag. Adopt only after relevant checks, independent differential controls and
source/artifact identity checks pass; otherwise retain the reference path.
