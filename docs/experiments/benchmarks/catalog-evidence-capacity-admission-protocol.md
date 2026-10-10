# Catalog capacity evidence reallocation and bound execution admission

Status: registered resource-only amendment before subsequent execution.
The original allocation review, frozen CR2 provenance and completed plain pair1
stay unchanged. All original fixtures, native binaries, metrics, independent
oracles, guards, required pairs and historical outcomes remain authoritative.

CR2 plain pair1 completed exactly in289.31s at approximately4.3GiB whole-job
peak. Retained run evidence is approximately141MiB, including35MiB shared raw
Batch objects and90MiB from two unique answer sets. Ten remaining children
project approximately450MiB more unique answers. Together with CR1 and CR3,
this exceeds the reviewed512MiB capacity allowance even after exact gzip.
All registered plain pair1 guards were true; storage admission does not change
performance acceptance or require rerunning this valid pair.

Reallocate256MiB from catalog headroom: capacity allowance is now **768MiB**,
query stays **1GiB**, aggregate catalog stays **2GiB**. Inventory retains unique
inode byte lengths, allocated blocks and separate logical path totals. Capacity
includes CR1/spill, CR2, CR3, lifetime and baseline-freeze trees when present.
Persistent evidence lives in the repository on `/home` (455GiB free observed);
build caches and temporary fixtures use the mounted data drive (437GiB free
observed). These are distinct storage locations. Keep original8GiB scratch,
16GiB drive reserve,16/20GiB cgroup RAM limits, zero swap, time/stage/shared
execution budgets, CPU/demand and acceptance gates. No new native build.

## Immutable freeze plus explicitly bound replacement driver

The original freeze archived the driver before this resource-only update.
Optional `--execution-admission FILE` authorizes ONLY the exact reviewed
replacement driver. Without it, a driver SHA mismatch still fails closed.
Root writes one new admission manifest after registering the final driver and
this protocol. It binds the actual original provenance file SHA, original and
replacement driver SHAs, all four unchanged native binary SHAs, unchanged
profile/oracle/compaction helper SHAs and this reviewed protocol SHA.
No original receipt or archive is edited. Plain pair1 retains its original
provenance; subsequent slices copy the admission and its source SHA into their
own provenance, together with the actual selected execution driver SHA.

The exact JSON schema is (replace every SHA from the actual indicated file;
`binary_sha256` and verifier entries are copied verbatim from original freeze):

```json
{
  "version": 1,
  "scope": "resource-only capacity evidence reallocation",
  "freeze_provenance_sha256": "SHA256(RUN/freeze/provenance.json)",
  "original_driver_sha256": "freeze.source_sha256[tools/bench/labs/catalog/borrowed_log.py]",
  "replacement_driver_sha256": "SHA256(current tools/bench/labs/catalog/borrowed_log.py)",
  "binary_sha256": {
    "plain-baseline": "original SHA", "plain-candidate": "original SHA",
    "counted-baseline": "original SHA", "counted-candidate": "original SHA"
  },
  "unchanged_verifier_sha256": {
    "tools/bench/labs/completion/profile.py": "original SHA",
    "tools/qualification/query_oracle.py": "original SHA",
    "tools/bench/labs/catalog/compact_evidence.py": "original SHA"
  },
  "capacity_evidence_bytes": 805306368,
  "aggregate_catalog_bytes": 2147483648,
  "reviewed_protocol_sha256": "SHA256(this registered protocol file)"
}
```

Use the original registered six-pair commands from the slicing protocol,
adding `--execution-admission RUN/execution-admission.json` to each remaining
pair and the aggregate command. Never rerun plain pair1 merely for allowance.
Aggregator accepts that original slice only with its exact embedded original
freeze; amended slices must carry the same validated admission and execution
SHA. All original native hashes, trial metrics,64 unique passed full-chain
verdicts per child, cleanup and six required pairs remain checked. It requires
768 complete chains before any nomination; partial resource admission is no
performance result. Altered freeze/driver/native/verifier/protocol bindings
are injected as controls and must be rejected before measured native work.

CR3 is not yet frozen; its imported capacity inventory and local guard use the
same768MiB allowance, while its fixture/binary/metric/acceptance logic is fixed.
No workload, builds, tests or validators ran during amendment preparation.
