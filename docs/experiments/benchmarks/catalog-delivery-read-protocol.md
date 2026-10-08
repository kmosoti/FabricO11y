# Reuse validated delivery positions: registered screen

Status: registered before execution. This is an algorithm and observer ablation,
not deployment qualification. It continues the [overlap results](coupled-completion-run-01.md#continuation)
without altering their outcomes or the product contract.

## Mechanism and competing explanations

The collector reads and CRC-checks a durable frame for delivery, discards its
next position, then rereads the frame after ACK to recover that position.
The candidate keeps one private `(sequence, next_position)` in `Spool` and
remaps it during rotation. After the same durable ACK-marker write, it can
advance the first acknowledged frame without another payload read. Further
frames in a cumulative ACK still follow the existing path. Cache memory is
constant; wire bytes, sync ordering, one in-flight request and retry bytes stay
unchanged. A poisoned writer cannot use the shortcut.

This challenges the assumption that deeper preparation queues are the next
useful optimization. Alternatives are a persistent send worker (thread overhead),
a deeper queue (latency hiding but more queued custody), and a post-commit
observer (general instrumentation). First remove demonstrably redundant work.

H1: at 256 and 896 KiB, cached ACK advancement has at least 10% lower median
wall time in each of three matched warm-cache pairs. H0: any pair misses this
criterion. The mechanism control must count 32 ACK frame reads for baseline,
zero for candidate. Read/decode timing, complete-loop CPU ticks and wall time
remain separate: reduced ACK work is not a throughput prediction.

H2: moving the finite fixture's observer out of the loop reduces node CPU by
at least 25%. H0: it does not. The current observer scans and decodes the whole
retained Spool on every 5 ms loop, even when idle. `FABRIC_O8_OBSERVER=final`
captures it once after draining, including its cost in process CPU. This mode
requires a fresh Spool retaining every sequence from 1 through the final commit;
missing/reclaimed prefixes fail. Existing independent custody/query oracles and
missing/duplicate controls are unchanged. This is a fixture correction, not a
production collection optimization.

## Semantic controls and limits

Run the existing Spool and overlap controls plus new paired cache controls:
empty/retry, public default, ACK jump/future ACK, one/multiple rotations and
frame-end boundary, marker failure/reopen, poisoned writer and failed rotation.
Record a differential post-read corruption cut: after a valid delivery read,
externally damage that committed frame before ACK. The baseline's second read
can report corruption while the candidate can accept the already-delivered ACK.
Both paths persist the ACK marker before traversal. This is a changed error
observation, not byte/error equivalence; single-owner immutable committed frames
are an explicit assumption. No production promotion is authorized by this
screen. Record the counterexample and retain the default-off selector.

## Fixtures and commands

The ignored `spindle::spool::read_probe` test uses deterministic seed 42 log
bodies of 16, 256 and 896 KiB, 32 durable frames per arm and three pairs per size,
alternating arm order. Fixture creation is outside timing. Per-frame read and
ACK wall times and complete-loop CPU ticks are retained; checks outside timing
establish exact stored bytes, sequences and final ACK/reopen. These bodies are
valid storage envelopes; the large single body is not a claim about the native
newline collector's 4 KiB line limit. Scratch is disk-backed and removed only
on success. Warm-cache timings do not measure cold disk or network capacity.

Build two release node examples from the same source, `FABRIC_ACK_ADVANCE_EXPERIMENT=0`
and `=1`, `FABRIC_BORROWED_LOG_EXPERIMENT=0`, with unrelated spill/run/shared
selectors unset. Freeze and read back each compressed executable and its hashes.
Reuse the exact server and dump archives from `catalog-overlap-freeze-01`, with
their original hashes verified. Record the working tree, compiler commands,
environment and source hashes.

Use the unchanged `coupled_overlap.trial(..., profile_name='backlog')` and its
independent oracles: 512 preloaded 4000-byte log bodies (seed 42), 20 seconds per
node, 50 ms relay delay, 8 MiB Spool. Run serial delivery in this order:
baseline/loop, baseline/final, candidate/final, baseline/final repeat.
The two final baselines bound order drift; this is one candidate service sample,
not a p99/capacity or repeatable speedup claim. Report source-to-Spool/ACK,
Spool-to-ACK, node CPU per logical MiB, accepted Batches/bytes, retries/errors,
RSS/cgroup memory, CPU/I/O counters, observer scans and all query verdicts.
If candidate CPU or ACK median exceeds either baseline by 10%, record a service
preservation miss; no default promotion even when these guards meet.

Root runs every workload serially through:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-delivery-read-01 --lab coordinator --stage preparation --seconds 900 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 20 -- python3 -B tools/bench/labs/catalog/delivery_read.py
```

The driver formats only touched Rust, runs scoped controls with both compile
selectors, runs the ignored release probe, freezes nodes and executes the four
service arms. Exact child commands/exits go into its receipt. Full fast and
manual documentation profiles run subsequently with separate contained receipts.

## Resource admission and cleanup

Use the unchanged 20 GiB max / 16 GiB high / no-swap outer cgroup, 30-minute
launcher deadline, mounted data-drive scratch and build cache. Each server is
384/320 MiB max/high with 2 CPUs; each node 64/48 MiB with 1 CPU. Their delegated
parent is bounded too. No remote work. This task consumes the existing frontier
(3345 seconds remained before this task), never resets it.

Reserve 20 MiB conservatively before the driver. Its own evidence cap is 16 MiB
including node archives; coordinator source receipts and later checks count in
the unchanged aggregate/query caps. Remove successful fixture scratch per arm.
On failure, stop all owned children; exact archived executable copies may be
removed only after matching their retained archive hashes/decoded bytes, with
a reference receipt. Keep remaining failed fixtures for the existing lossless
cleanup workflow. Preserve failure records rather than retrying over them.

The final record must separate actual mechanism savings, instrumentation costs,
end-to-end observations and untested corruption/production implications.
