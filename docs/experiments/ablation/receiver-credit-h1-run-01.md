# H1 packet-slot ablation: sender windows and receiver credits

## Status and scope

**Registered before comparison execution on 2026-09-26; first result recorded below.** This is the first runnable, network-limited H1 cell of the broader [receiver-driven transport study](receiver-driven-transport.md). It compares a fixed sender window (M0) with receiver credits (M1) for synchronized 64 KiB incast. It is a deliberately simple discrete-time Rust model, not a Homa or SIRD implementation and not a measurement of a Fabric network service. The existing [Stage 6 log baseline](../benchmarks/local-log-stage6.md) supplies application context, but no disk operation occurs in this network-limited cell.

## Hypothesis and fixed contract

At matched injection-interval goodput, M1 will reduce the p99 burst-peak switch-plus-receiver queue bytes by at least 10% relative to M0. A packet sent with M1 must consume exactly one receiver credit; all granted but not yet delivered scheduled packets together may not exceed nine packets. A producer retains its message until a modeled post-commit durable ACK returns. The network-limited receiver commits a fully arrived message immediately in the model, then sends that ACK. CREDIT and packet receipt ACK are control signals, not durable ACKs. This study does not infer physical persistence from a modeled commit.

Both variants use the same immutable message trace, packetization, sender and switch caps, link rate, propagation times, and delivery rule. Each message has identity `(producer, sequence)`, where `producer` is `0..31` and `sequence` is burst index plus one. In the valid normal run there is no intentional packet loss. Switch-cap overflow or an incomplete message by the common deadline fails the trial rather than silently yielding a favorable latency. Packet loss recovery and overload behavior belong to later experiments; this first cell cannot support a reliability claim under loss.

## Registered model and trace

| Parameter | Value |
| --- | --- |
| Producers / receiver / switch | 32 producers, one switch egress, one receiver |
| Message | One 65,536-byte batch per producer in each burst; 1,000 bursts per seed; 32,000 offered messages per seed |
| Packet | At most 1,500 data bytes; final partial packet consumes one full slot; 44 packets/message |
| Link clock | One switch egress packet per tick; one sender packet per tick; tick represents 1.2 µs at 10 Gbit/s for a full 1,500-byte packet |
| Delay | Sender-to-switch data: four ticks; receiver-to-sender control: four ticks. The modeled credit round trip from issue to the next possible receiver delivery is nine ticks including one serialization slot. |
| BDP / M0 window / M1 grant budget | Nine packets. M0 gives each sender a nine-packet unacknowledged receipt window independently of active sender count; M1 limits **all** granted but not yet delivered scheduled packets to nine. |
| Byte caps | Sender queue: 131,072 unsent payload bytes per producer. Retained source copies until durable ACK: 131,072 payload bytes per producer. Switch queue: 768,000 payload bytes. Network-limited receiver has no waiting admission queue; it consumes completed messages immediately. |
| Control | M0 returns one 32-byte packet receipt ACK per delivered data packet. M1 sends one 32-byte announcement per message and one 32-byte credit per granted packet. Both send one 32-byte modeled durable ACK per completed message. Control consumes neither switch data-egress slots nor a separately contended control link; its bytes are reported. |
| Switch schedule | FIFO among arrived packets; process arrivals, sample queued bytes, then transmit at most one packet per tick. At most one packet per producer can be sent each tick. |
| M1 grant schedule | At announcement arrival, grant available packet credits across announced messages in fixed round-robin producer order. On each packet delivery, replenish one credit to a message with unsent ungranted packets if possible. A grant reaches its producer four ticks after issue. |
| Trace release | Burst `0` releases at tick `0`. For burst `i >= 1`, release tick is `i × 1760 + ((SplitMix64_next(seed) mod 801) − 400)` using an unsigned 64-bit SplitMix64 state initialized to the seed. All 32 messages in a burst release together. Ties use producer order. |
| Seeds and interval | Evaluation seeds `0..9`, paired M0/M1 runs. The injection interval is half-open `[0, 1,760,000)`; a completion at its end tick is outside the achieved-goodput count. The common drain deadline is tick `1,770,000` inclusive. The 1,408,000 full-packet slots offered per seed equal 80% of injection slots. |

These values freeze the first cell. There is no calibration or parameter selection in this run. The model omits sender-uplink contention between destinations, a shared core, switch priorities, ECN, CPU cost, and real file sync. This is why it addresses H1 only; M2–M7 and sink-limited results remain separate work.

## Metrics and advancement rule

For every message, record release, first wire-send, complete receiver arrival, and modeled durable-ACK receipt ticks. **Network completion latency** is complete arrival minus release, including sender waiting. Count data bytes completed by injection end for achieved goodput, and all trace bytes for offered goodput over the same interval. Record every message's completion/ACK status at the common deadline. Record control bytes, peak and time-integrated sender and switch queue bytes, peak retained-source bytes, packet loss, and receiver queue bytes (zero by model construction). A burst's hot-spot peak is the maximum switch-plus-receiver queued bytes from its release until all 32 messages complete; compute each seed's p99 by nearest rank across 1,000 bursts. Also compute per-producer p99 and maximum network completion latency across its 1,000 messages, and per-producer injection-interval goodput.

Before ranking, require every seed and variant to complete all 32,000 messages **and return their modeled durable ACKs** by the common deadline, complete at least 95% of offered bytes by injection end, have zero cap overflow and silent loss, send no ACK before modeled commit, preserve stable identities, and maintain the nine-credit invariant. Require M0/M1 achieved goodput within 5% in each paired seed. The candidate must keep every producer's p99 network completion latency within 5% of M0 and each producer's maximum within twice M0. A smaller switch queue alone does not establish smaller total memory: report sender queued bytes, retained copies, and sender-plus-switch byte-time separately. If any correctness or goodput gate fails, report H1 as inconclusive at this configuration.

For seeds passing those gates, compute each seed's relative p99 hot-spot reduction `(M0 − M1) / M0`; zero M0 occupancy makes the relative result undefined. The preliminary H1 target advances only if the mean of the ten paired reductions is at least 10% and the lower endpoint of a deterministic 10,000-resample paired bootstrap 95% interval is above zero. Resample seed pairs with replacement using Python `random.Random(42)` and percentile endpoints. Report every seed's metric and the interval. Even an advancing simulation result does **not** select a transport protocol; real-host durable-ACK latency, CPU, loss behavior, and workload representativeness remain gates in the parent study.

## Execution and validation

The intended runnable tool is [tools/transport-sim](../../../tools/transport-sim/). The [runner](../../../tools/bench/run_transport_h1.sh) executes tests, the registered ten paired comparisons, independent [CSV analysis](../../../tools/bench/summarize_transport_h1.py), and source/environment/checksum capture:

```sh
bash tools/bench/run_transport_h1.sh target/transport-h1/run-01
```

The output directory must be new. Preserve message-level and burst-level raw CSV, run environment, source hashes, exact toolchain, analyzer command, and a checksum manifest before interpreting results. Check an analytic single-message path, packet accounting, byte caps, the nine-credit invariant, and a synchronized incast case. Inject an overgrant defect and require a checker to fail. The [formal transport model](../../../formal/transport/) is intended to check the credit and ownership rules in a finite state space before treating any simulator result as evidence. If implementation details force a change to a registered value, revise this record before the comparison command and note the change explicitly.

## Executed run and results

`bash tools/bench/run_transport_h1.sh target/transport-h1/run-01` exited **0** on 2026-09-26 local time. Its six Rust tests passed, the optimized build completed, the ten M0/M1 seed pairs ran, the [independent analyzer](../../../tools/bench/summarize_transport_h1.py) exited 0, and all five original output checksums verified. The [preserved raw directory](data/receiver-credit-h1-run-01/README.md) contains the [summary CSV](data/receiver-credit-h1-run-01/summary.csv), [burst CSV](data/receiver-credit-h1-run-01/bursts.csv), [compressed message CSV](data/receiver-credit-h1-run-01/messages.csv.gz), [analysis JSON](data/receiver-credit-h1-run-01/analysis.json), [environment and source hashes](data/receiver-credit-h1-run-01/environment.txt), and checksum manifests. The environment record hashes this experiment document **before** this results section was added; the model and analyzer hashes identify the executed code.

Each seed offered 32,000 messages (2,097,152,000 payload bytes) and completed all of them before the half-open injection interval ended. Every modeled durable ACK arrived by the drain deadline. Both variants had zero rejection, loss, cap overflow, or unacknowledged message. M1 reached exactly nine granted-but-undelivered packets, its budget; M0 reached its nine-packet **per-sender** window. Achieved injection-interval payload goodput was identical at 1,191.56 bytes/tick (about 79.4% of the 1,500-byte data-slot capacity); producer goodput was identical at 37.236 bytes/tick. These values include no contended control link or real sink service.

| Metric, per seed unless noted | M0 sender window | M1 receiver credit |
| --- | ---: | ---: |
| p99 burst-peak receiver-plus-switch queue | 420,000 B | 13,500 B |
| Peak switch queue | 420,000 B | 13,500 B |
| Mean switch queued byte-ticks across ten seeds | 533.17 billion | 2.14 billion |
| Mean sender queued byte-ticks across ten seeds | 985.83 billion | 1,533.63 billion |
| Mean total sender-plus-switch queued byte-ticks | 1,519.00 billion | 1,535.78 billion |
| Mean peak unsent sender bytes, aggregate | 2.28 MB | 2.71 MB |
| Peak retained-source bytes, aggregate | 4,194,304 B | 4,194,304 B |
| Control bytes | 46,080,000 B | 47,104,000 B |
| Per-producer p99 network completion across seeds | 1,689–1,756 ticks | 1,697–1,764 ticks |

The p99 hot-spot reduction was **96.7857% in every one of the ten paired seeds**. Its paired mean and deterministic 10,000-resample bootstrap interval were both exactly 96.7857%; this zero-width interval reflects identical modeled hot-spot peaks across the jitter seeds, **not** certainty about an actual network. Each producer's p99 network-completion latency increased by eight ticks, at most 0.474% relative to its M0 value; the maximum-latency guardrail also passed. The preregistered completeness, goodput, resource, latency, and bootstrap gates therefore mark this **preliminary H1 model cell as advancing**. This is not adoption of receiver credits in Fabric O11y.

## Interpretation and limits

The model shows exactly the trade-off it was built to expose: a shared nine-packet receiver budget prevents the many independent sender windows from filling the switch. Switch peak drops by 96.8%, while mean sender queued byte-time rises by 55.6% and mean total sender-plus-switch queued byte-time rises by 1.1%. M1 also sends 1,024,000 more control bytes per seed (2.22% above M0), from one announcement per message. A lower hot-spot peak here is **not** a claim of lower total memory or cost.

This result depends on a FIFO, one-packet-per-tick switch; fixed four-tick data/control delays; unlimited, uncontended control delivery; no packet loss; immediate modeled receiver commit; and one repeated synthetic 64 KiB burst shape. The exact same hot-spot peak in all ten seeds means the jitter changed other queue integrals but not this primary metric. The queue-peak CSV contains one aggregate per burst and no per-tick queue timeline, so the analyzer can check endpoints, caps, and p99 agreement but cannot independently reconstruct every peak. A tiny incast unit test checks one peak analytically. The separate [finite TLA+ check](../formal/transport-credit-ownership.md) proves modeled safety only within its smaller state space and does not prove the Rust simulation or durability of an actual transport.

The next transport cells need an unscheduled-prefix sensitivity case, priority/active-grant attribution, and sender/shared-link feedback under a shared-bottleneck topology. A sink-limited model using the measured Stage 6 service trace, loss and retry behavior, and real-host durable-ACK validation are still required before making a Fabric transport choice. The [parent study](receiver-driven-transport.md) keeps those requirements distinct from this first H1 result.
