# H1 packet-slot simulator

This standalone Rust package runs the registered M0 sender-window and M1 receiver-credit comparison for the 32-to-1 incast trace. It is a deterministic scheduling model, not Fabric O11y network transport. A completed message gets an immediate **modeled commit** at the receiver; no EventLog or disk operation occurs.

```sh
cargo test --manifest-path tools/transport-sim/Cargo.toml --offline --locked
cargo run --release --manifest-path tools/transport-sim/Cargo.toml --offline --locked -- h1 <fresh-output-dir>
```

The output directory must not exist. The command writes `summary.csv`, `messages.csv`, and `bursts.csv`, grouped by seed `0..9` and variant `M0`, `M1`. Message rows are ordered by producer then sequence within each group. All numeric quantities are data payload bytes or ticks unless a column names packets. `control_bytes` counts 32 bytes per announcement, grant, packet receipt ACK, or modeled durable ACK, without control-link contention.

At each tick, the model releases a burst, handles due control and data events, issues M1 grants, samples queue occupancy, lets each sender send at most one packet, and transmits at most one FIFO switch packet. Data reaches the switch four ticks after send; an egress transmission during tick `t` reaches the receiver at `t+1`. Control reaches a producer or receiver four ticks after issue. Thus a grant issued at tick `t` permits a send at `t+4`; with an idle switch, delivery occurs at `t+9`. A completed message commits in the same receiver tick, and its durable ACK reaches the producer four ticks later.

Sender queue bytes are **unsent** payload, sampled before sender wire-send. Retained full message copies are separately sampled and held until durable ACK. Both per-producer quantities have the same 131,072-byte cap. Switch bytes are sampled after packet arrivals and before egress. Each burst's hot-spot peak spans its release through the tick when all its messages complete, including any overlapping bursts; the receiver admission queue is zero by construction. The injection interval is half-open `[0, 1_760_000)`, and the deadline tick `1_770_000` is included.

The run fails on queue-cap overflow, receiver-credit overgrant, packet-accounting error, ownership error, or incomplete delivery/ACK at the deadline. There is no packet loss, retransmission, rejection policy, switch priority, CPU model, or real durability in this first cell.
