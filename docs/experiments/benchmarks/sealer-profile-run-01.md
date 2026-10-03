# Sealer profile run 01: where the server's CPU goes, and two removals

Status: **Exploratory.** Measured on 2026-10-03 on a 4-CPU, 15 GiB Ubuntu 24.04 container (CPU with SHA extensions). No protocol was registered; nothing here is **Measured** in the [evidence-state](../../QUALIFICATION.md#evidence-states) sense. Follows [ingest run 01](ingest-run-01.md) (sealing is the server's ingest bound at about 27 MB/s per core) and [spindle run 01](spindle-run-01.md) (one node delivers 3.6 MB/s, 77 ms per Batch).

## Question

[Ingest run 01](ingest-run-01.md) showed sealing bound by CPU and [spindle run 01](spindle-run-01.md) showed one node bound by the time a Batch spends at the server. Both numbers were measured from outside. This run looks inside: an instruction profile of the sealer, and an accounting of a lone node's 77 ms.

## Method

1. **Instruction profile.** The release server (commit before this run's changes) sealing one real-text 64 MiB journal file ([a3lite](optimality-run-01.md)) with `seal_workers=1` under `valgrind --tool=callgrind` ([profile.sh](data/sealer-profile/profile.sh.txt)); the server is terminated once the sealed file is gone. Output: [self cost](data/sealer-profile/callgrind-self.txt), [inclusive cost](data/sealer-profile/callgrind-inclusive.txt). Instructions, not time: `perf` is not installable here.
2. **Native sealing time.** The same file sealed natively by the release server pinned to one CPU, wall time until the sealed file is gone and the process's CPU seconds, three runs each ([seal_time.sh](data/sealer-profile/seal_time.sh.txt)); both include replaying the 64 MiB journal at open.
3. **A lone node's Batch.** The cost of the server's group-commit window read from the code ([store.rs](../../../crates/fabric-server/src/store.rs)) and the Spool's and journal's syncs per append ([frame.rs](../../../crates/fabric-frame/src/frame.rs)), with the cost of one `fsync` on this disk measured directly: 4.6 ms after a 1 MiB append, 0.04 ms with nothing dirty.

## Results

### The profile (27.1 G instructions in all)

| Where | Share of all instructions | What it is |
| --- | ---: | --- |
| `segment::build` | 75.2% | sealing the file |
| of which `Digest::digest` (SHA-256, software) | 32.3% | one digest per Batch for the custody table and one per written file |
| of which `write_text_filter` → `GroupFilter::build` | 24.4% | the trigram filter: a `HashSet<u32>` insert per trigram (38 M inserts, SipHash) |
| of which `write_table` (Arrow and Parquet, Zstd 3) | 20.1% | the three tables |
| of which `batches_batch` (prost decode of every Batch) | 13.7% | decoding, with a copy of each `logs` field |
| of which sorting rows | 12.6% | `sort_by_key` over the row structs |
| `Store::open` replaying the journal | 22.5% | `identify` decodes every Batch in full and `digest` hashes it again, only to learn its Strand and sequence |

Two readings need care:

- **SHA-256 is a valgrind artifact in size, not in kind.** callgrind does not emulate the CPU's SHA extensions, so the `sha2` crate fell back to its software path. Natively this CPU hashes at 1.35 GB/s (`openssl speed`), so the roughly 150 MiB hashed per 64 MiB file costs about 0.1 s of the 3.5 s: a few percent, not a third. The hashing is still doubled at replay.
- **SipHash is not an artifact.** The filter's hash set costs the same instructions natively. Removing the SHA share, the filter was about half of what `segment::build` spent on this file.

### Two removals, measured natively

| Build | Seal one 64 MiB file, wall s (3 runs) | CPU s | MB/s of journal, one core |
| --- | ---: | ---: | ---: |
| before (HEAD) | 2.79, 2.79, 2.85, 2.95 | 2.70, 2.70, 2.79, 2.85 | 24.4 |
| after (filter by bitmap, quiet rule) | 2.37, 2.47, 2.48, 2.58 | 2.32, 2.37, 2.40, 2.46 | 28.1 |

Four interleaved pairs (head, after, head, after, …) on the same CPU; the rate is 64 MiB over the median CPU seconds, replay included. An earlier pass of this A/B had measured a stale binary and is discarded.

**The filter by bitmap.** `GroupFilter::build` now marks each trigram as one bit in a 2^24-bit map (2 MiB per build) and reads the set bits back, instead of inserting into a hash set. The filter bytes are identical for every input (the same set of trigrams goes through the same positions), so every existing filter test and fixture stands unchanged. Natively the saving is about 13% of the sealer's CPU (2.75 to 2.39 s at the medians, replay included), not the 24% of its instructions: the hash set's instructions were cheap ones on cached data, while the time goes to Parquet, Zstd, copying and the file system, which callgrind weighs lightly. Instruction share is a map of where to look, not of what will be saved.

**The quiet rule.** The node's timestamped stdout ([batch_anatomy.py](data/sealer-profile/batch_anatomy.py.txt) over `node.out`, 30 s of the deliver mode) and an `strace` of its socket calls put the numbers on spindle run 01's 77 ms per Batch: 78 ms between consecutive Batches, of which 63 ms (p10 61, p90 67) was the node waiting in `recvfrom` for the server's answer and 12 ms was collecting, encoding and appending the next Batch to the Spool. The server's work on a 1 MiB Batch is a few milliseconds of decoding and hashing plus its journal syncs, so about 50 of the 63 ms were the group window: with one Batch in flight per Strand, a lone node's submission could never be joined by a second one, and every Batch waited the whole window. The group now also closes when nothing has arrived for 2 ms ([ADR-0013](../../decisions/ADR-0013-deliver-batches-in-order-with-bounded-dedup.md), amended). Under load, arrivals at 500 hosts come about every 0.5 ms, so groups still fill to 1 MiB or the window. Durability is unchanged: a group is synced before any answer leaves.

With the rule, the same dissection gives 27 ms per Batch: 13 ms (p10 12, p90 17) from send to answer and 12 ms of collection and Spool append. The two halves are now the same size, and they are serial: the next lever for one node is to collect the next Batch while the previous one is in flight, which needs no protocol change, and after it the six syncs per Spool append and the server's two per group.

| Mode (spindle run 01 setup, 60 s) | Before (spindle run 01) | After |
| --- | ---: | ---: |
| deliver: log text, MB/s | 3.63 | 9.7 (the 256 MiB log drained in 27.76 s; node CPU about 40% of a core while busy, 11.44 s in 27.76 s) |
| deliver: Batches in 60 s | 779 | 964 in 27.76 s |
| deliver, capped at 4 MiB/s: MB/s | 1.57 | 1.57 (the cap, not the server, is the limit) |

[spindle2 data](data/sealer-profile/spindle_bench.json).

## What was not changed, with its size

- **Replaying the journal at open** decodes and hashes every Batch (22.5% of this profile's instructions; proportional to the journal). A header-only `identify` that walks the protobuf wire and skips the payload fields, and trusting the digest the Group record already carries, would remove most of it. It touches the replay of a durable format and is left as a scope item.
- **Decoding copies the payloads.** prost decodes `Batch.logs` into a fresh `Vec` and then the OTLP bodies into fresh `String`s: two copies of every byte before a row exists. `bytes::Bytes` fields in the envelope would remove the first copy; it changes the envelope's Rust types across crates, not its bytes.
- **Sorting rows by key** sorts the full row structs (12.6%); sorting indices or keys with a payload offset is the usual remedy and is part of [ADR-0022](../../decisions/ADR-0022-build-segments-by-external-merge-sort.md)'s rebuild.
- **The wire format of a log line** carries five string attributes per record (path, device, inode, two offsets): 175 of a 324-byte record for a 130-byte line (2.49×, computed; 2.7× measured with framing). Moving path, device and inode to the scope and the two offsets to integer values gives 1.62×; a start offset alone 1.38×. This is a wire-format change and needs explicit scope.
- **The trigram filter under high-entropy text** (base64 payloads: 257 k distinct trigrams per row group against about 8 k for the corpus) saturates at the 2^20-bit cap, where one trigram passes with probability 0.15 instead of 0.014; a six-byte needle still passes with probability 0.003. The cap bounds the filter at 128 KiB per 1 MiB group. No change needed.

## Threats to validity

- Instruction counts are not time; the native A/B is the time measurement, on one file, one core, including replay.
- The quiet rule's fault runs ([delivery and recovery](../../milestones/delivery-recovery.md)) were not rerun; the rule changes when a group closes, not what is synced before an answer, and the store's two unit tests cover the timing claim only.
- One machine, one corpus (15,995 distinct lines repeated).
