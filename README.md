# FabricO11y

FabricO11y is a Rust telemetry server with Linux collectors and a browser console.
It collects logs, host metrics and application traces, durably forwards them to
a central server, and answers queries with explicit freshness and missing-coverage
information.

## Current product

- **Spindle** (`fabric-node`): collects CPU, memory, filesystem, disk, network and selected log files on one Linux host as OpenTelemetry protobuf inside versioned Batches, and keeps them in a durable local **Spool** until the server acknowledges them. Unreadable sources and full disks become visible gaps.
- **Delivery**: authenticated TLS; one Batch in flight per **Strand** (one Spindle generation's ordered lineage); the server acknowledges only after a durable commit, deduplicates retries and rejects conflicting bytes.
- **Fabric Server**: a durable journal, immutable Zstd Parquet **Segments**, retention by age and size, and log, metric and counter-rate **queries** whose answers report completeness, freshness, collection gaps and the retained window, with snapshot-bound pages.
- **Console and access**: a Leptos/WASM PWA with a local owner passkey and read-only reader tokens for scripts and AI clients. Explore telemetry and manage Spindles in the console; `fabricctl access` lists nodes and queries with a reader token.
- **Self-observation**: each server supervises its own dedicated Spindle. Edge Spindles send their diagnostics to their configured server.

The bounded release for development and small deployments is in acceptance
testing. Debian and Fedora packages are being verified with the console and
100 GB retained-telemetry default. No release has been tagged and no deployment
profile is qualified. See [current evidence](docs/CURRENT.md), the
[release plan](docs/milestones/release-readiness.md) and the
[release epic](https://github.com/kmosoti/FabricO11y/issues/33).
The [research wiki's progress page](https://github.com/kmosoti/FabricO11y/wiki/Release-readiness-progress)
links measured results and the active acceptance queue.

## The main idea

A pure, `no_std` semantic core decides what a transition means (for example, whether a Batch commits, duplicates, conflicts or reveals a gap); ports describe effects; adapters perform them; independent verification decides whether the implementation deserves the claim. The dependency rule and the core's purity are checked by `cargo xtask`, and independent Python oracles grade delivery and query behavior.

## Build and run

The tested toolchain is Rust 1.99.0, including its WASM target. Follow the
[contributor setup](docs/CONTRIBUTING.md#resource-containment) for the storage root
and enforced cgroups:

```sh
python3 -B tools/resource_group.py -- cargo xtask checks --profile fast
python3 -B tools/resource_group.py -- cargo build --release --locked --workspace --bins
```

[Operating FabricO11y](docs/operations.md) covers server/edge setup;
[access operations](docs/access-operations.md) covers passkeys and recovery.
[Package construction](packaging/README.md) includes the required console build.
The original [FOL2 demonstration](https://github.com/kmosoti/FabricO11y-research/blob/main/docs/architecture/fol2-demo.md) is preserved in the [research archive](https://github.com/kmosoti/FabricO11y-research).

## Explore

- [Architecture documentation](docs/README.md): landing page, source-of-truth order and reading order.
- [Current state](docs/CURRENT.md) and [roadmap](docs/ROADMAP.md).
- [Product contract](docs/PRODUCT-CONTRACT.md) and [qualification](docs/QUALIFICATION.md).
- [Verification strategy](docs/formal/verification-strategy.md) and [verification matrix](docs/formal/verification-matrix.md).
- [Experiments](docs/experiments/README.md): registered protocols, results and research tooling (research is not product behavior).
- [Learning path](docs/LEARNING_PATH.md) and [contributing](docs/CONTRIBUTING.md).
- [Report a bug](docs/CONTRIBUTING.md#reporting-bugs) or
  [report a security vulnerability privately](SECURITY.md).

Licensed under [Apache-2.0](LICENSE). Dependency terms are recorded separately in
the [dependency policy](docs/dependency-policy.md) and packaged notices.
