# FabricO11y

FabricO11y is an observability system in Rust built around one question: *what did reality tell us, what happened to that evidence, and how much of it can we truthfully claim to know?* It collects host telemetry, preserves its meaning and custody across failures, retains it under explicit policy, and answers queries without hiding missing coverage or uncertainty.

## Current product

- **Spindle** (`fabric-node`): collects CPU, memory, filesystem, disk, network and selected log files on one Linux host as OpenTelemetry protobuf inside versioned Batches, and keeps them in a durable local **Spool** until the server acknowledges them. Unreadable sources and full disks become visible gaps.
- **Delivery**: authenticated TLS; one Batch in flight per **Strand** (one Spindle generation's ordered lineage); the server acknowledges only after a durable commit, deduplicates retries and rejects conflicting bytes.
- **Fabric Server**: a durable journal, immutable Zstd Parquet **Segments**, retention by age and size, and log, metric and counter-rate **queries** whose answers report completeness, freshness, collection gaps and the retained window, with snapshot-bound pages.
- **Central control**: enrollment, desired and applied configuration, pause, resume and revoke through `fabricctl` and an admin HTTP API.

Maturity: implemented and tested; the target operating profile (one controlled Debian-family installation, Debian 12/13 or Ubuntu 22.04/24.04 and derivatives, natively or under WSL2, up to 1,000 simulated identities) is **not yet qualified**. Several registered measurements passed on earlier revisions; history latency, freshness, outage/drain, stress, soak and running installation are outstanding, and no release has been tagged. See [qualification](docs/QUALIFICATION.md).

## The main idea

A pure, `no_std` semantic core decides what a transition means (for example, whether a Batch commits, duplicates, conflicts or reveals a gap); ports describe effects; adapters perform them; independent verification decides whether the implementation deserves the claim. The dependency rule and the core's purity are checked by `cargo xtask`, and independent Python oracles grade delivery and query behavior.

## Build and run

With Rust 1.85 or newer (CI pins 1.98.0):

```sh
cargo test --workspace --locked            # tests, including the architecture gates
cargo xtask checks --profile fast          # every required fast check, with receipts
cargo build --release --locked             # fabric-node, fabric-server, fabricctl
```

[Operating FabricO11y](docs/operations.md) covers configuration, enrollment and recovery. The original single-process **FOL2 demonstration** still runs with `cargo run -- 42 3` ([FOL2 demonstration](docs/architecture/fol2-demo.md)); it is a legacy learning path, not the product.

## Explore

- [Architecture documentation](docs/README.md): landing page, source-of-truth order and reading order.
- [Current state](docs/CURRENT.md) and [roadmap](docs/ROADMAP.md).
- [Product contract](docs/PRODUCT-CONTRACT.md) and [qualification](docs/QUALIFICATION.md).
- [Verification strategy](docs/formal/verification-strategy.md) and [verification matrix](docs/formal/verification-matrix.md).
- [Experiments](docs/experiments/README.md): registered protocols, results and research tooling (research is not product behavior).
- [Learning path](docs/LEARNING_PATH.md) and [contributing](docs/CONTRIBUTING.md).
