# Native receipt verification: finite measurements

Alpha acceptance was paused for this engineering work. Synthetic receipts below
are explicitly marked: their commands were not executed. They test the verifier,
not product acceptance. The [receipt protocol](verification-receipts-protocol.md),
[representation contract](../../formal/verification-receipt-contract.md),
[hash ablation](verification-hash-backend-protocol.md) and
[provider discrimination](verification-crypto-provider-protocol.md) preceded runs.

At clean source `a5c834728d8c667f7422e94a2bf63fd91a4f0589`, both default
RustCrypto and optional OpenSSL variants completed their differential/benchmark
drivers with actual exit 0 and confirmed whole-tree stops. Each matched the
unchanged Python reference on 21 common decisions and deliberately rejected four
representation aliases that Python accepted. Positive reports matched after
normalizing environment observations. Earlier scalar failures and slower results
remain retained; these measurements do not retroactively qualify them.

## Measured provider comparison

The CPU was an i7-10750H with AVX2 and no SHA instruction flag. Each corpus had
ten logs of the listed size, one warmup per implementation and seven alternating
Python/native pairs. Timing includes process startup, Git/registry checks and
complete log hashing; fixtures and compilation are outside timing. OpenSSL and
default batches ran in that fixed order, rather than interleaved with each other.
All files were on the mounted DATA volume under the required resource launcher.

| Bytes per log | Default native median | OpenSSL native median | Python median in OpenSSL batch |
| --- | --- | --- | --- |
| 64 B | 15.462 ms | 15.914 ms | 59.052 ms |
| 64 KiB | 18.298 ms | 17.112 ms | 60.408 ms |
| 4 MiB | 171.136 ms | 94.132 ms | 134.623 ms |

The ten-large-log native ratio of medians fell 45.0%; OpenSSL's median paired
reduction relative to Python was 30.2%. The tiny native case regressed 2.9%, or
0.453 ms. Native maximum RSS medians were about 6.6 MiB, Python about 21.2 MiB.
GNU time CPU observations have 0.01-second precision and are not wall times.
These are warm finite local measurements, not a universal engine ranking.

Python and the candidate resolved the same local libcrypto 3.5.9 file. Agreement
between them alone is therefore not an independent crypto oracle. Known vectors
and RustCrypto cross-checks across ten sizes and ten chunk boundaries separately
checked the adapter; all three digest tests exited 0. The feature's full 56-test
suite exited 0 before a test-only typed-array adjustment; afterward the three
digest tests, all-target/all-feature Clippy, formatting and release build exited
0. Default crypto remains portable and unchanged unless the explicit feature is
enabled. The earlier optional assembly backend improved the large native case
only about 9.2% and remained slower than Python; it was not adopted.

## Evidence and remaining work

DATA-relative provider report:
`results/verification-provider-ablation-01/actual-comparison-01.json`, SHA-256
`fcccc4f879ffd1cf1e72fcb6d0baa4bc8207a0c4df3a4c68ac0757d428a7191a`.
It pins both driver reports, wrapper exits/stops and archived ELF identities.
OpenSSL differential completed at 2026-10-18T02:58:59.933206Z; default completed
at 03:00:10.239411Z. Full-input digest verification has an Omega(B) byte-reading
bound; changing the provider reduces constant cost, not the work required for
integrity.

The [isolated hash-comparison mutant](verification-receipt-mutant-protocol.md),
production dispatcher integration, complete required checks, architecture/docs
validation and explicit workflow adoption remain necessary. No acceptance,
security certification, general optimality or release completion follows from
these experiments.
