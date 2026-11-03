# Representation and distributed custody evaluation

Compare current JSON, schema-directed binary rows and bounded dictionary columns
without changing production SDK admission, v2 byte-exact retries or local custody.
A canonical experimental record is a validated Observation; retain integer widths,
finite f64 bits, optional fields, strings and structured values without loss.
Every frame identifies its experimental schema. Reject unknown schema, malformed
lengths, invalid references, trailing input and excessive expanded content.

A batch contains at most 32 observations and 256 KiB canonical JSON. Dictionaries
are self-contained, at most 1024 entries and 64 KiB UTF-8 bytes; overflow uses inline
strings. Encoded frames are bounded at 1 MiB. Decode must bound counts and expanded
strings before allocating them. Baselines receive identical batching opportunities.

Spindle Seal means local durable custody; Fabric Seal means server durable custody.
Neither is a cryptographic signature. Remote identities include persistent origin,
producer, generation and sequence. Duplicate delivery is idempotent; conflicts
cannot overwrite records. A confirmed frontier cannot skip a missing sequence.
Model safe reclamation eligibility without enabling production eviction. Fabric
Weave is reserved for later partitioning; one durable server is the present model.

Use two Rust programs: representation evaluation and distributed fault evaluation.
Qualify candidates with independent fixtures and adversarial tests before ranking.
Exercise restarts, message loss/reordering/duplication, lost receipts, full stores,
conflicts, schema/dictionary mismatches and exact recovery. Mathematical durability
assumes atomic stable transactions; implementation tests are not refinement proofs.
Liveness requires eventual communication, fair processing and available capacity.

Measure sender, Spindle and receiver CPU separately and summed; allocation traffic,
peak live allocations and peak RSS separately; wire/storage bytes, setup/recovery,
throughput, materialization/projection costs and both seal latencies. Timing runs
must disable allocation instrumentation. CPU and RSS include native dependencies;
Rust allocator counters must disclose excluded native allocations. Use release,
one warmup, five repetitions, two build jobs and sequential evaluation. Freeze
development/held-out fixtures and evaluator before selection. Record identities,
hardware, filesystem/cache and resource settings. Model 10/100/1000 Mbps and
1/20/100 ms RTT with sequential and pipelined limits; no two-host measurement claim.
No automatic winner, composite score, global memory optimum or whole-system proof.
