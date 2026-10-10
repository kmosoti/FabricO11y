# Oversized-line progress and owned log projection

Registered before executing the proposed reproduction or performance comparison.
Continue the [algorithm round](catalog-algorithm-round-findings.md) with two
separate questions. Core binding and query selection changes are held fixed.
No wire validity, oracle, source cursor, ACK ordering or sync boundary is changed.

## Collection progress: reproduce before correction

Hypothesis: after an oversized-line gap is committed, a log-only pass that reads
only the next part of that skipped line returns no Batch and discards prospective
cursor movement. The independent adapter may advance while runtime repeatedly
starts at the same committed byte. This would delay valid lines behind the
discarded line until another payload (usually a metric sample) permits a commit.

Fixture: one file containing exactly `3 * 1048576 + 10` ASCII `X` bytes followed
by `\nnormal\n`, total 3,145,746 bytes. Use fixed valid host fixture files, an empty
Spool and a 3,600-second configured metric interval. Call `collect_logs`, ACK each
committed Batch durably, and reopen after the second commit. Desired committed
offsets are 1,048,576; 2,097,152; 3,145,728; 3,145,746. The only log is `normal`,
with start 3,145,739 and end 3,145,746. Exactly one oversized-line gap is allowed;
no discarded suffix may become a log. Capture the real trace before assertions.

Run the desired regression against unmodified production collection first.
Expected counterexample: second pass returns `None`, cursor remains 1,048,576,
next sequence remains 2 and ACK remains 1. Preserve actual exit 101, trace, input
recipe/hash and the complete small compressed fixture. The driver may return 0
only after explicitly establishing that this exact expected counterexample was
observed; label the Rust check failed, not passed. An unexpected outcome requires
investigation before correction.

Candidate if reproduced: when an otherwise quiet pass proposes forward progress
from an existing oversized-skip cursor, collect real host metrics late in the
same pass, allowing a valid Batch to carry that cursor. Do not reread the file,
invent a new gap meaning, or admit cursor-only empty Batches. A host sample error
uses the existing real host-failure gap. Install replacement counter history and
source cursors only after successful durable append. Extra metric observations
are a documented cost, analogous to existing rotation-driven samples; no new
sampling cadence guarantee is introduced.

The unchanged regression must then pass. Also check full-Spool refusal preserves
cursor/history and marks unknown coverage, retry from the last committed cursor,
failed host sampling still makes progress with its real gap, ordinary quiet
files still make no Batch, and skip completion with an incomplete normal suffix
does not consume that suffix. Preserve the original counterexample separately.
This is a finite progress investigation, not an arbitrary-input liveness proof.

## Query/storage projection: remove only owned log copies

`rows::extract` decodes an owned OTLP tree but then borrows and copies log body,
attribute keys and string attribute values. Consume those owned log vectors and
move their Strings into unchanged row fields. Keep node-label copies, metrics,
spans, gaps, hex representation and decode order unchanged. This is separate from
storage pruning and from a future hex-formatting experiment.

H1: against an exact frozen legacy log extraction baseline, the median of three
candidate/legacy wall-time ratios is at most 0.95 in each 1024-byte-body cell and
at most 1.10 in each 16-byte-body cell. H0: any criterion misses. Fixtures use
seed 42, eight bounded Batches of 128 logs each, body sizes 16/1024 and zero/eight
string attributes. Three pairs alternate arm order, 20 whole-extraction repeats
per arm. Encode fixtures and check full output outside timing; time decoding,
projection and destruction as explicitly reported by the probe. Report all raw
pairs. Do not extrapolate to complete-query latency, RSS or ingestion throughput.

Correctness controls compare exact legacy output and independent explicit fields:
multiple resources/scopes, ordering/indices, empty/Unicode and every non-string
body kind, missing values, duplicate attributes (last string wins; a later
non-string does not erase it), ignored resource attributes, prepopulated output,
and malformed envelope/node ID/log/metric/trace payloads. Preserve partial output
when a later signal fails to decode. Mixed-signal numbers compare represented
bits for NaNs/signed zero and exact integer boundaries. A borrowed-clone control
must demonstrate copies while consuming projection preserves live String owners;
an incorrect duplicate-key rule must be rejected. Existing independent complete
query checks remain unchanged and run in the final fast profile.

## Dispatch and resource accounting

First complete the [exact protocol-copy reclamation](catalog-protocol-dedup-protocol.md).
Root alone dispatches these local jobs, serially, through `resource_group.py` and
`completion/run_job.py`. All descendants share verified containment, data-drive
build/scratch paths, 20 GiB max, 16 GiB high, no swap and the 30-minute outer limit.
No remote work or deployment qualification is included. Fresh job IDs:

| Job | Stage | Deadline | Prospective evidence reserve after coordinator snapshots | Driver output cap |
| --- | --- | ---: | ---: | ---: |
| `catalog-log-progress-repro-01` | preparation | 180 s | 1 MiB | 512 KiB |
| `catalog-log-progress-round-01` | preparation | 900 s | 2 MiB | 1 MiB |
| `catalog-log-progress-checks-01` | verification | 600 s | 1 MiB | 512 KiB |

Every command, exit, source hash and relevant exact source accompanies its result.
The first job archives its failed Rust fixture with member/byte readback before
removing scratch. Other successful owned fixtures are removed; unexpected failures
retain evidence under the launcher workflow. Final verification runs all fast
and manual documentation gates; its Bun copy uses the established exact existing
archive-reference cleanup. No frontier, stage or evidence limit is reset.
