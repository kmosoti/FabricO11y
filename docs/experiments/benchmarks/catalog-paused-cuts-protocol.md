# Paused publication, checkpoint and shared-reader cuts

Status: **registered before execution; no outcome claimed**. Extends
[operations planning](catalog-labs-operations.md) and
[coverage protocol](catalog-coverage-protocol.md). This authorizes no oracle
change, wire change, new durable lease, sync-order change or qualification.
Root must capture the final source/protocol before running the new target.

## Proposed real process fixture

Use the existing constructed seed `0xCA7A10A1` and pre-IO expected ledger:
eight groups, three nodes, late timestamps and equal-time ties, logs/metrics/spans.
Ancient receive times for groups 1–4 and current times for groups 5–8 distinguish
the later age-retention partition. Rotate groups 1–4 to pending journal label 1;
leave 5–8 active. Prime retained Scan, default Walk and shared-catalog Walk readers
through full pagination. Explicit refresh is derived-state maintenance only.

An actual owned helper process invokes the public bounded builder. Compile the
existing private [IO interposer](../../../tools/bench/labs/completion/io_fault.c)
inside containment; select `sync` on owned root + `manifest.json$` and pause mode.
Parent waits at most 30 seconds for the child to be stopped and for a matching
injection receipt; Segment must not be published. Query while builder is stopped:
pending and active coverage must match the prebuild ledger in all three readers.
Resume child and require successful bounded build; grade published/unreclaimed
coverage and verify the journal still exists.

Next an owned helper opens real Store/runs actual sealer pass. Pause its first
sync on owned `streams.json.tmp$` during startup reclaim; matching injection and
stopped child are required. Parent grades while published Segment and its journal
coexist, before stream checkpoint durability/reclaim. Resume, require successful
child exit and removed sealed journal, then grade cached readers across reclaim.
Never treat missing injection, timeout or unexpected child exit as a passed cut.

Close/rotate the active journal to label 5, publish/reclaim it through actual
Store/sealer, retain first-page tokens and reader objects, then age-evict Segment 1.
Require Segment 5 retained, old continuations Gone in all three variants, and new
full chains equal the independently partitioned retained ledger. Sharing an Arc
metadata handle retains memory; it cannot authorize an expired page or pin files.

H1: these selected publication/checkpoint cuts preserve exact coverage, read
answers and old-snapshot validity; logical eviction still produces Gone with
shared cached metadata alive. H0: any identity/order/envelope/reclaim/token
assertion differs. Independent full-chain oracle calls use unique directories
and explicit stage/variant context. Existing dropped/duplicated/filter/bounds
controls remain separate; do not infer exhaustive interleaving coverage.

## Cleanup and admission

Owned child RAII cleanup must kill/wait on unwinding, including stopped children;
the parent never waits indefinitely. Successful children must be resumed explicitly
and exit within a finite timeout. Keep stderr injection traces, operation/path,
child exit, stage ledger/pages and resource/cleanup receipt. Never inject in the
parent, ledger writer, independent grader or unrelated paths. No unsafe production
code or new shared filesystem fault is introduced.

Root executes only after source/protocol capture, using mounted data-drive owned
scratch through the 16/20 GiB, zero-swap launcher and serial coordinator. Proposed
900-second command timeout, 1 GiB intended scratch, 16 GiB free reserve, 32 MiB
compact evidence. Stop on missing control receipt, mismatch, OOM, clock issue,
deadline, storage ceiling or surviving child. Archive failures before cleanup.
Record command/exit, cgroup peak/events, scratch/evidence bytes and child cleanup.

Prepared target selector (final source must agree before registration):
`paused_publication_and_checkpoint_preserve_shared_reader_coverage_and_gone`
in `catalog_interleavings`. Run with `cargo test --offline --locked -p fabric-server
--test catalog_interleavings paused_publication_and_checkpoint_preserve_shared_reader_coverage_and_gone
-- --exact --test-threads=1 --nocapture`, inside the coordinator/launcher, with
`FABRIC_STORAGE_EVIDENCE` pointing to its fresh evidence directory.
The `paused_catalog_child` test is ignored by ordinary runs and invoked only by
the parent's explicitly filtered helper command. Do not use `--include-ignored`.
Owned child exit codes are archived separately for build/checkpoint stages.

Limits: paused writer/process plus an executing reader exercises selected real
schedules; it does not enumerate arbitrary races, mid-read deletion, every
checkpoint/reclaim instruction or physical power loss. Typed internal source
handles are exercised through shared History; no public handle-lifetime theorem
or IO pin guarantee is claimed. Partial-table availability remains separately
blocked by the whole-record oracle model and needs explicit trust-boundary design.
