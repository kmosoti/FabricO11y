# Cost harness review: final-c source

Verdict for these exact sources: REQUEST_CHANGES. No formal timing run was performed.

Reviewed `/tmp/fabric-cost` at `079120fe2ec7c0aa80d6f6ddded50c83f182b739`, with these six SHA-256 hashes (also in `/tmp/fabric-cost-harness-result.json`):

| File below `/tmp/fabric-cost` | SHA-256 |
| --- | --- |
| `tools/storage-probe/src/bin/common/mod.rs` | `fe1c2127ea72430698e9fb1f4da5f918b21e5f6c5529435a919f6e79e003f7cb` |
| `tools/storage-probe/src/bin/receipt-cost.rs` | `b227f52b4c4e6f8c9b2f58427774372e4a40402dca54a902200cc542a4fb97c7` |
| `tools/storage-probe/src/bin/sidecar-cost.rs` | `37fd986a1a78b71d0e05ef56b56b0efb316ac566fb304a7ef654b8dea01d38b8` |
| `tools/layout-probe/src/bin/layout-cost.rs` | `765aa6092ec2f5647357eee7e1373434651bf04a651c70eea3f14de9dc7b225c` |
| `tools/bench/run_research_costs.py` | `87e257c7ba17b1c7c04abc29f92be5afcbca7fb19fd32198ce91724d2e5b363c` |
| `tools/bench/test_research_costs.py` | `e5ba85bf6e7f3a3fe0b81cdd2616568bef7707e8cabcb4c378a98477eaad2d5f` |

## Findings and inputs

1. **Decision field is trusted without checking the raw query samples.** Using `mixed-201-2048-0-zstd64/result.json` from `/home/kmosoti/fabric-cost-smoke-final-c-20260927`, I changed only `p50_projected_ns` to `100000000`. The unchanged 128 projected samples have nearest-rank p50 `2330300`. `checked(path, 'layout', 'smoke', 'mixed', 201, 0, 384)` accepted the altered result, and `summarize(...,[('mixed',201,2048)],[0])` flipped `registered_layout_gate` from `True` to `False`. The exact fixture is `summary-fixture/mixed-201-2048-0-zstd64/result.json` here. A bad derived value can reverse the registered decision while all answer and source hashes still agree. Source: `run_research_costs.py` lines 37-50 and 78-96.

2. **Result structure checks also admit impossible observations.** With the same zstd64 smoke result, `checked` accepted three separate inputs: replacing sample 127 with a duplicate of sample 0 (thereby omitting that query), setting sample 0 `timing.wall_ns=-999`, and setting sample 0 `matches=999999`. The retained inputs are `duplicate_query.json`, `negative_time.json`, and `wrong_match_count.json` here. These values can bias query-family summaries while the run still claims `checks_ok=true`. Source: `run_research_costs.py` lines 43-49.

3. **The S3/S4 persisted-byte and amortization boundary is incomplete.** For zstd64, `layout-cost.rs` lines 130-160 writes only `table.parquet`; the `TableAnchor` needed for full-file authentication stays in memory and its serialized bytes are absent from `layout_bytes`, `metadata_bytes`, and durable publication. Lines 200-210 time `build_postings`, then write and sync `postings.json` and sync the parent outside that timer. The index digest used by `query_postings` also stays in memory. `run_research_costs.py` lines 85-96 derives the break-even count from only `index_build.wall_ns`. The registration requires full-file authentication, index external hash, and byte accounting that includes needed metadata/index copies. The current result therefore describes a resident trusted anchor/index, with no recorded byte or publication cost for retaining their trust bindings. The protocol's phrase "index build cost" is ambiguous about durable publication; the parent has clarified that the final boundary will time pure build and durable index/digest publication separately and use their sum for amortization. No formal performance result was produced from the current code.

## Executed checks

- `python3 -B tools/bench/test_research_costs.py /home/kmosoti/fabric-cost-smoke-final-c-20260927`: exit 0. Its four built-in mutations were rejected.
- The supplied final-c smoke metadata reports `status=complete`, 27 commands, all exit 0, on `/dev/sdd ext4 /`. It has one warmup and one measured trial for the 2,048-event mixed workload; it is not formal timing evidence.
- Compiled `fail_fsync.c` with `gcc -shared -fPIC ... -ldl`: exit 0. With `FAIL_FSYNC_AT=3` and `LD_PRELOAD=fail_fsync.so`, `layout-cost smoke mixed 201 2048 99 json64 ... layout-fsync-fail` exited 1 with `Input/output error`, showing the publication file-sync failure propagates. With `FAIL_FSYNC_AT=5`, `layout-cost smoke mixed 201 2048 100 zstd64 ... index-fsync-fail` also exited 1 on the index file sync. Neither wrote a success result.
- S1 source comparison: `common::workload` and `common::queries` for the 2,048-event mixed and shuffled-Log cells match `tools/storage-probe/src/main.rs` tenant, timestamp, Log body, and eight-predicate cycle. The 8,192-event path changes the timestamp modulus to 8,192 and repeats the cycle fourfold (512 queries). The smoke outputs contain 128 query results per method for receipt and layout, as expected.
- The receipt query loop checks positions and full row digests after each operation, and the new `black_box(&scalar)` keeps construction observable. Sidecar phase timing includes source read/parse, hint construction and write, and server read/parse plus validation. The sidecar and layout results report logical source/file sizes separately from Linux kernel I/O counters.
- Brief CLI B review: `tools/bench/run_prototype.py` builds release/locked, invokes actual separate CLI processes, checks expected exit status, retained output hash, retry idempotence, partial/resume closure, missing manifest rejection and rebuild, and wrong-snapshot rejection. No CLI B defect found in this pass; I did not rerun its 21-process lifecycle.

VERDICT: REQUEST_CHANGES
