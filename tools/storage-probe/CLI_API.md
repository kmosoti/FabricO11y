# Research lifecycle CLI contract

Status: registered before CLI implementation. Add a separate `fabric-research`
binary to storage-probe; retain the existing S1 runner. It composes FOL2, S2 and E3,
accepts caller-owned version-1 event JSON, and never substitutes fixed demo answers.
All commands use positional arguments below; invalid usage exits 2, operation
errors exit 1 with a concise stderr explanation. Successful commands emit one JSON
object to stdout. Valid incomplete query/resume is exit 0 with `complete: false`.

```text
fabric-research generate SEED COUNT FRESH_INPUT
fabric-research adapt-otlp REQUEST_JSON CONFIG_JSON FRESH_INPUT
fabric-research ingest INPUT LOG CAPACITY BATCH
fabric-research publish LOG FRESH_SNAPSHOT FRESH_TRUSTED_PUBLICATION BLOCK_ROWS SNAPSHOT_ID
fabric-research query SNAPSHOT TRUSTED_PUBLICATION QUERY_JSON AVAILABILITY FRESH_CHECKPOINT
fabric-research resume SNAPSHOT TRUSTED_PUBLICATION QUERY_JSON CHECKPOINT CHECKPOINT_SHA256 AVAILABILITY FRESH_CHECKPOINT
fabric-research verify INPUT TRUSTED_PUBLICATION QUERY_JSON CHECKPOINT CHECKPOINT_SHA256
fabric-research rebuild SNAPSHOT TRUSTED_PUBLICATION
```

`generate` is convenience input creation only: generator Events, with even physical
positions changed to Log bodies `common request{position} rare` for positions divisible
by 7 and `common request{position}` otherwise; odd positions retain Gauge. Serialize
using S2 codec and sync a fresh file plus its existing parent. The other commands
accept independently created inputs with any Event values; do not assume this shape.
All file inputs are capped at 64 MiB before parsing; never accept a truncated prefix
of oversized input. All fresh outputs reject existing files/directories/symlinks.
Parent directories must already exist and be durable. Preserve source and failed
outputs. Numeric parameters use checked conversion; CAPACITY/BATCH/BLOCK_ROWS >0.

`ingest` decodes the entire bounded input before opening LOG. The existing FOL2 log
must be an exact bit-preserving prefix of input (same_record_contents); reject wrong
source or extra stored rows without appending. Feed only the remainder through
EventBuffer with the requested logical capacity and drain batch size. Retain caller
ownership until each borrowed append returns success. Record `input_events`,
`already_committed`, `appended`, `buffer_retries`, `peak_buffer_events`, `capacity`,
`batch`, and `log_bytes`. An identical retry appends zero and preserves every byte;
duplicate EventIds and identical records at distinct positions are retained. The
64 MiB request cap bounds input bytes, while EventBuffer bounds queued events, not
all memory or per-event bytes. On storage error stop and state that caller must
rebuild from its independent source on healthy storage before retrying that path.

`publish` requires LOG already exists; opening a missing path must not create an
empty authoritative snapshot. Replay FOL2, call S2 publish with a fresh immutable
name, then save the returned Publication at a caller-owned path outside the snapshot
directory. Reject a trusted output located within that snapshot before publication;
its existing parent must resolve outside the final snapshot path. No success report
before both steps sync. An unadvertised orphan after failure is not a trusted root.
Output includes `publication` and `snapshot` path. New arrivals create a successor
snapshot with a different caller-chosen ID; old checkpoints stay bound to their root.

QUERY_JSON contains exactly Query's four required fields: `start_ns`, `end_ns` (i64),
`tenant` (null or u64), `token` (null or string). Unknown/missing fields are rejected.
Duplicate object members are also rejected, before writing a checkpoint.
AVAILABILITY is `all`, `none`, or a 0/1 string of exactly block_count bits (empty string
is valid for an empty snapshot). Construct Binding from the independently loaded
Publication, exact query and supported tokenizer/order versions.

`query` reads S2, validates the page through Accumulator, saves one-page history in a
fresh checkpoint, and outputs `binding`, `complete`, `unavailable` (ascending
unresolved ordinals), `rows` (MatchedRow list), `positions`, `reads` (S2 ReadMetrics),
`checkpoint_sha256` (64 lowercase hex), `checkpoint`, and `history_pages` (1).
The caller retains the returned digest independently from that checkpoint file.

`resume` loads the checkpoint using the explicitly supplied digest and expected
Binding, asks S2 only for its derived residual, merges atomically, appends the new
page to history and writes a fresh checkpoint. Output has the same fields as query
and updated history_pages. Earlier checkpoint bytes are unchanged. Wrong anchor,
query, version, hash or corrupt history fails with no new checkpoint. All bits true
cannot make missing/corrupt candidate rows Complete. Safe exclusions need no raw I/O.

`verify` independently decodes INPUT, rebuilds its SealedSnapshot anchor from the
trusted Publication's block_rows and snapshot_id, checks exact anchor equality,
loads checkpoint against caller digest/Binding, and requires Complete. Evaluate
inclusive time, exact tenant and Unicode-whitespace Log tokens using a separate
scalar loop (not the optimized query code). Compare every position and full Event
digest against accumulated rows. Output `verified: true`, `matched`, `input_events`.
An incomplete but otherwise valid checkpoint fails verification. It never mutates
source or checkpoint, and never consults untrusted metadata as its expected anchor.

`rebuild` uses S2's same-root reconstruction and reports the newly written manifest
path. It does not manufacture a replacement root. Cold placement is exercised by
moving a retained block to the local `cold/` directory in the reproducible demo;
there is no claim about object storage or physical power loss.

An end-to-end test must invoke real separate processes for ingest, publish, query,
resume and verification. Use new external input, exact retry, wrong source, partial
availability, cold placement, raw loss and restoration, a late event/successor,
checkpoint tampering, wrong-query reuse, and independent final result comparison.
Record all command exits, output hashes and source versions. Root application's
normal CLI and FOL2 format remain unchanged.

`adapt-otlp` follows the separately registered [collection profile](COLLECT_API.md).
It writes the same version-1 Event input that ingest accepts and reports
`adapted_events` and `output` only after the fresh output and parent are synced.
