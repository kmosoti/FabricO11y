# One unscheduled packet before receiver credit

## Status and question

**Registered before implementation and comparison execution on 2026-09-26 local time; run 01 completed on 2026-09-27 UTC.** This is the next network-limited mechanism cell of the [receiver-driven transport study](receiver-driven-transport.md). It compares M1 (metadata-only request, then receiver-granted data) with M2 (one immediate DATA packet carrying the message announcement, then receiver-granted remaining data). The [first H1 result](receiver-credit-h1-run-01.md) fixes the existing M1 simulator behavior. This cell asks whether removing the request/credit startup from a tiny message improves its p99 network completion without making a 32-producer 64 KiB incast exceed a named hot-spot budget.

The motivation comes from the unscheduled/scheduled split in the [Homa paper](https://people.csail.mit.edu/alizadeh/papers/homa-sigcomm18.pdf) and the [SIRD paper](https://www.usenix.org/system/files/nsdi25-prasopoulos.pdf). M2 is a Fabric research **mechanism model**, not either full protocol. It has no priority queues, congestion feedback, real packets, disk service, or physical durable ACK.

## Fixed semantics and arithmetic bound

M1 sends a 32-byte metadata-only request at release. It reaches the receiver after four ticks; the receiver issues credits; a credit reaches its sender after another four ticks. Every M1 DATA packet consumes one credit. M2 sends its first DATA packet at release when its sender can transmit. The first packet carries a 32-byte announcement in the same data slot. The receiver learns the identity and total size **only when that packet is delivered through the switch**; it then grants credits for remaining packets. Each later M2 DATA packet consumes one credit. Both variants retain the full source copy until a modeled durable ACK, sent only after immediate modeled receiver commit of the whole message, reaches the sender four ticks later. CREDIT and a packet receipt never release ownership.

The common data slot holds at most 1,500 **wire** bytes. In M2 the first packet holds up to 1,468 payload bytes plus 32 announcement bytes; subsequent packets hold up to 1,500 payload bytes. M1 packets hold up to 1,500 payload bytes and send the announcement separately. Both registered message shapes still use one or 44 data-packet slots, respectively. `offered_bytes`, completed goodput, and sender-queue bytes are payload bytes; switch-queue bytes and `data_wire_bytes` include M2's embedded announcement. Report 32-byte control-packet bytes separately from embedded announcement bytes, and their sum as modeled overhead. Control packets have fixed four-tick delay but do **not** contend for data slots or a modeled control link. Neither variant gets a hidden zero-time announcement.

The receiver's global budget of nine outstanding *scheduled* grants remains unchanged. M2's first packet is intentionally outside it. With 32 synchronized 64 KiB messages and one 1,500-byte unscheduled wire packet per sender, all 32 packets can reach the switch in the same tick before egress: **32 × 1,500 = 48,000 bytes** of queue, even with zero scheduled grants. A switch cap below 48,000 bytes must overflow for this isolated burst under the registered event order. A cap above it does not prove safety when scheduled packets or bursts overlap. This is a bound on the model's first arrival wave, not a measured network result; a tiny executable incast probe must check it.

## Registered traces and model settings

Every seed is paired across M1 and M2. Use the existing deterministic SplitMix64 trace rule: burst `0` releases at tick `0`; burst `i >= 1` releases at `i × spacing + ((SplitMix64_next(seed) mod (2 × jitter + 1)) − jitter)`, with the RNG state initialized to the `u64` seed. All producers in a burst release together in producer order. Identities are `(producer, burst + 1)`. Use evaluation seeds `0..9` and no calibration or post-result tuning. The half-open injection interval counts completions at ticks `< end`; the drain deadline includes its end tick.

| Setting | Tiny telemetry profile | Large-batch incast profile |
| --- | ---: | ---: |
| Producers → receivers | 8 → 1 | 32 → 1 |
| Payload bytes/message | 148 | 65,536 |
| Data-packet slots/message in both variants | 1 | 44 |
| Bursts/messages per seed | 1,000 / 8,000 | 1,000 / 32,000 |
| Nominal burst spacing / jitter | 10 / ±3 ticks | 1,760 / ±400 ticks |
| Injection end / inclusive drain deadline | 10,000 / 11,000 ticks | 1,760,000 / 1,770,000 ticks |
| Payload bytes offered per seed | 1,184,000 | 2,097,152,000 |
| Sender unsent and retained-copy cap, each per producer | 592 payload bytes | 131,072 payload bytes |
| Switch queue cap | 768,000 wire bytes | 768,000 wire bytes |

Both profiles retain the [H1 model's](receiver-credit-h1-run-01.md) four-tick sender-to-switch delay, four-tick control delay, one data packet per sender per tick, one FIFO switch egress packet per tick, nine-packet global receiver-credit budget, zero receiver waiting queue, immediate modeled commit, and no intentional data/control loss. In valid normal trials a cap overflow or unfinished message/ACK is a failed run; it is never silently dropped. Offered full-packet slots are 80% of each injection interval. The switch queue is sampled after due data arrivals and before one egress transmission; a burst's hot-spot peak spans its release through the tick its last message completes, inclusive. Unsent sender bytes and retained-source bytes are reported separately.

M1 under the large-batch profile must reproduce the preserved [H1 M1 message timings](data/receiver-credit-h1-run-01/messages.csv.gz) for all seeds and identities, as well as its p99 burst hot-spot of 13,500 bytes. A mismatch invalidates the comparison until explained. This is a regression gate on the control, not a new result.

## Metrics and decision rule

Record each message's release, first send, complete receiver arrival, modeled commit, and durable-ACK receipt ticks. Network completion is `complete − release`, including sender waiting; durable-ACK latency is `ACK receipt − release`. Compute nearest-rank p50 and p99 **within each seed and profile**, with all offered messages included. Record per-producer p99 and maximum network completion, injection-interval completed payload bytes, control packet bytes, embedded announcement bytes, data wire bytes, scheduled and unscheduled packet counts, sender/switch peak and byte-time, retained-source peak, p99 burst-peak switch-plus-receiver queued wire bytes, cap overflow, loss/rejection, and pending ACKs. Preserve message and burst rows, environment, source hashes, commands, and checksums.

Correctness and comparability come first. Each variant must complete every offered message and modeled durable ACK by the common deadline, complete at least 95% of offered payload bytes before injection ends, and have zero cap overflow, rejection, loss, duplicate identity, or early ACK. M1 and M2 achieved injection-interval goodput must differ by at most 5% in every paired seed. M2 must send exactly one uncredited first DATA packet per message; all later packets require credits and the global scheduled-grant count must never exceed nine. The sender's unsent and retained-copy caps are both enforced. If any gate fails, the profile is inconclusive at these fixed settings.

The **primary small-message target** is M2 versus M1 at the tiny-telemetry profile's 80% offered packet-slot load: each seed's p99 network-completion relative reduction `(M1 − M2) / M1`. It advances only if the mean paired reduction over seeds `0..9` is at least 10% and the lower endpoint of a deterministic 10,000-resample paired-bootstrap 95% interval is above zero. Resample seed pairs with replacement using Python `random.Random(42)` and nearest-rank interval endpoints. A zero M1 p99 makes the relative target undefined and inconclusive.

The **incast guardrail** is M2's p99 burst-peak switch-plus-receiver queue at most 65,536 wire bytes in every seed, zero cap overflow, and every producer's p99 network completion at most 5% above its M1 control (maximum at most twice M1). This is a fixed exploratory byte budget, not a claim of hardware safety. Report the M1→M2 change in sender queued byte-time, switch queued byte-time, total queued byte-time, retained-source bytes, and modeled overhead even if the primary target passes. An improvement in tiny-message completion that fails the incast guardrail does not advance M2. Advancing both cells does not select a Fabric transport; the broader M2 prefix-0/BDP sensitivity, mixed-size priority, active-grant, sender/core feedback, sink-limited, loss, and real-host cells remain separate.

## Intended execution and validation

The [Rust simulator](../../../tools/transport-sim/README.md) will add a distinct M2 path without changing H1's M0/M1 command. A [new runner](../../../tools/bench/run_transport_m2.sh) will use a fresh output directory, execute the simulator's tiny checks, run both profiles on the ten paired seeds, invoke an independent [CSV analyzer](../../../tools/bench/summarize_transport_m2.py), and save source hashes and checksums:

```sh
bash tools/bench/run_transport_m2.sh target/transport-m2/run-01
```

Before the comparison, check the one-message timing path, 1,500-byte boundary packetization, sender retention, scheduled-credit budget, the 32-packet first-wave queue and an injected switch cap below 48,000 bytes. The latter must fail visibly. Preserve H1 M1 timing equality as a separate regression check. If a registered value must change, revise this document before running the ten-seed comparison and explain the change. The existing [finite transport model](../formal/transport-credit-ownership.md) checks scheduled-credit and ACK order for a smaller state space; it does not model the uncredited prefix or prove this Rust code.

## Execution and validation

From the repository root, `bash tools/bench/run_transport_m2.sh target/transport-m2/run-01` exited **0**. It ran `cargo test --offline --locked --manifest-path tools/transport-sim/Cargo.toml` (six library tests and five independent M2 contract tests passed), generated 40 profile/seed/variant runs, ran the independent CSV analyzer, and checked all five original run-file SHA256 hashes. The host and exact source hashes, including this document's **preresult** hash, are in [environment.txt](data/unscheduled-prefix-m2-run-01/environment.txt). The [preserved data directory](data/unscheduled-prefix-m2-run-01/README.md) contains 800,000 message rows and 40,000 burst rows, plus the full [analysis.json](data/unscheduled-prefix-m2-run-01/analysis.json).

Before that comparison, the release H1 command regenerated M0/M1 output. Its **10 M1 summary rows, 10,000 M1 burst rows, and 320,000 M1 message rows matched the preserved H1 files exactly**, row for row; the Python comparison exited 0. The M2 one-message contract checks sent a 148-byte payload at tick 0, delivered and modeled-committed it at tick 5, and received the modeled durable ACK at tick 9. M1's corresponding ticks were 8, 13, and 17. A one-burst 32-producer test observed exactly 48,000 wire bytes at the switch before egress; a 47,999-byte switch cap returned `switch queue cap overflow`. Its scheduled-grant maximum was nine. A separate test checked that a sent prefix did not release the retained source before ACK.

The analyzer independently rebuilt release ticks and identities, checked timing, accounting, caps, fixed control protocol counts, all message and burst rows, and every registered gate. It found **zero gate failures**. After a copy of the real output had one M2 `data_wire_bytes_sent` value increased by one, the analyzer exited **1** with `disagrees with protocol accounting`; the unmodified run remained untouched. An independent verifier also changed a copied M2 grant maximum from nine to ten, and the analyzer exited **1** with `scheduled global credit budget breached`.

## Results

Nearest-rank p99 values below are calculated within each seed, then averaged over seeds `0..9`; each seed includes all offered messages. All ten incast seeds had the same burst-peak p99 for each variant. The injection-interval completed payload bytes matched within the registered 5% gate in every seed. Every message and modeled durable ACK completed by the shared deadlines, with no modeled rejection, loss, cap overflow, or pending ACK.

| Registered cell | M1 control | M2 one-packet prefix | Decision gate |
| --- | ---: | ---: | --- |
| Tiny telemetry mean p99 network completion | 22.5 ticks (seed range 22–23) | 14.5 ticks (14–15) | Mean paired relative reduction **35.57%**; deterministic 10,000-resample paired-bootstrap 95% interval **[35.10%, 36.05%]**, above the 10% target |
| Tiny telemetry mean p50 network completion | 17 ticks | 9 ticks | Reported context |
| Tiny telemetry p99 burst hot-spot | 1,184 wire bytes | 2,160 wire bytes | Diagnostic; the tiny-cell target is latency |
| 32-producer incast mean p99 network completion | 1,733.4 ticks | 1,725.1 ticks | Every producer met the p99 `≤ 1.05 × M1` and maximum `≤ 2 × M1` guardrails |
| 32-producer incast p99 burst hot-spot | 13,500 wire bytes | **48,000 wire bytes** | Every seed stayed below the 65,536-byte budget |

The immediate prefix moved occupancy toward the switch. In the tiny profile, mean switch queued byte-time rose **149.2%** while mean sender queued byte-time fell **91.3%**; mean total sender-plus-switch queued byte-time fell **49.7%**. In incast, mean switch queued byte-time rose **44.6%**, sender queued byte-time fell **1.06%**, and total queued byte-time fell **1.00%**. The incast retained-source peak was **4,194,304 payload bytes** for each variant, equal to two 65,536-byte copies at each of 32 producers. For tiny telemetry the mean aggregate retained-source peak fell from **3,552** to **2,664 payload bytes**. These are queue and modeled-copy counts, not real process allocation measurements.

The mean modeled protocol overhead, separate control bytes plus embedded announcement bytes, fell from **768,000 to 512,000 bytes** per tiny seed and from **47,104,000 to 46,080,000 bytes** per incast seed. Embedded M2 announcement bytes are already part of `data_wire_bytes_sent`, so adding them again to data wire bytes would double-count them. The complete per-seed costs and goodput are in [analysis.json](data/unscheduled-prefix-m2-run-01/analysis.json).

## Interpretation and limits

**Both registered cells advance M2 to the next research question.** The tiny-message p99 reduction passed its paired target, and the incast buffer and per-producer latency guardrails passed at fixed settings. This does **not** select M2 as an application transport. Its gain follows partly from removing a modeled eight-tick request/credit startup, while its incast switch peak is over three times M1's. The 48,000-byte first wave is exactly the bound predicted by the one-packet-per-sender arithmetic under this synchronized trace; a larger or overlapping unscheduled wave still needs a separate cap analysis.

The simulator has no real network, loss/retransmission, priority queues, control-link contention, sender/core feedback, receiver disk service, CPU cost, or physical durable ACK. Its ACK follows immediate **modeled** commit; it is not evidence that the application's [EventLog](../../architecture/storage.md) has a network ACK. The analyzer checks CSV totals and timings, but the CSV alone cannot prove every packet consumed a credit or reconstruct every per-tick queue peak. Focused Rust probes cover those transitions for small cases; no formal M2 prefix model has been checked. The broader study still requires prefix-size and credit-budget sensitivity, mixed sizes, sink-limited and loss cases, and real-host validation before any protocol decision.
