# Remote sender concurrency discriminator

Before executing the simulated edge cases, the owner's requested network probe
measured 44.6 ms median application RTT and 41.4 Mbps remote→local payload
throughput. The original two-worker workload offers 100 ten-log Batches/s.
The RTT-only upper bound `workers / RTT` predicts about 44.8 requests/s for two
workers, before TLS, configuration polling and durable ACK work. Eight workers
raise that bound to about 179.4/s without changing the offered bytes.

Run the registered two-worker simulator, then a fresh eight-worker case on
digitalocean-02 with the same 100 identities, source seed, 60-second 1,000-log/s
offer, native 60-second drain allowance, 128 MiB memory, 50%-CPU, zero-swap and
disk limits. No new time or evidence allowance is allocated. Both fit the
current pressure round's ledger. Stop on an ACK-custody or oracle failure;
unacknowledged backlog with a successful delivery oracle is an overload result
that admits the concurrency discriminator rather than a data-loss finding.

H1: more independent requests reduce final backlog and increase accepted Batches
per second within unchanged CPU/RAM, indicating RTT-limited progress. H0: CPU,
server sync, configuration polling or another shared limit dominates. Report
actual attempted/ACKed/created rates, request and creation-to-ACK distributions,
remaining pending Batches, CPU throttling/RSS and independently checked custody.
Every final query chain still uses the unchanged Python oracle, including
missing/changed result controls. The simulator's pending state is simulated
in memory; it does not establish native Spool durability for unacknowledged
offers. A finite burst plus drain is not sustained-throughput qualification.
Two then eight is one ordered diagnostic pair, not a general concurrency default.
