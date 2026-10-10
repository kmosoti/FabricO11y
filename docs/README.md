# Architecture documentation

FabricO11y is a Rust observability system: a Spindle on each Linux host collects host metrics, logs and exported traces into a durable Spool and delivers them over TLS to Fabric Server, which commits before acknowledging, retains history as a journal and Parquet Segments, and answers queries that report what is complete, fresh and missing. The architecture is hexagonal: a pure `no_std` semantic core, effect ports, application use cases, adapters and thin composition roots, with the dependency rule checked by `cargo xtask`.

<!-- diagram: diagrams/system.mmd -->
```mermaid
flowchart LR
    subgraph Host[Linux host]
        Sources[procfs, statvfs and selected log files]
        Spindle[Spindle: fabric-node]
        Spool[(Spool: FAB1 frame log)]
        Sources -->|bounded reads| Spindle
        Spindle -->|Batch committed before cursor moves| Spool
    end
    subgraph Central[Fabric Server]
        Intake[HTTPS intake]
        Commit[Commit thread: delivery use case]
        Journal[(Server journal: FAB1)]
        Sealer[Sealer and retention]
        Segments[(Zstd Parquet Segments)]
        Query[Query]
        Control[(Control state)]
        Intake -->|exact bytes| Commit
        Commit -->|grouped two-sync append| Journal
        Journal -->|sealed files| Sealer
        Sealer -->|manifest last| Segments
        Sealer -->|published prefix before and between build groups| Commit
        Commit -->|checkpoint then oldest journal reclaim| Journal
        Journal -->|unsealed tail| Query
        Segments -->|snapshot-bound reads| Query
    end
    Spool -->|oldest unacknowledged Batch over TLS| Intake
    Commit -->|ACK after durable commit| Spindle
    Control -->|desired configuration poll| Spindle
    UI[Leptos PWA] -->|passkey session| Access[Scoped console API]
    CLI[fabricctl] -->|workload credential| Access
    Access -->|authorized mutation| Control
    Access -->|authorized sources and signals| Query
    CLI -->|local inspect| Spool
```

Each production server also owns a [dedicated diagnostic Spindle](architecture/system.md). Edge Spindles keep their configured destination.

The expanded [release plan](milestones/release-readiness.md) requires a
[Leptos operator PWA](milestones/operator-console.md) and
[local passkeys/scoped access](architecture/identity-access.md). The integrated
implementation is in candidate verification; the diagram does not establish
release acceptance. [Access operations](access-operations.md) covers local owner
setup, scoped CLI credentials, passkeys and recovery.

## Source of truth

The repository owns product and operator documentation, architecture and executable
verification. The [research wiki](https://github.com/kmosoti/FabricO11y/wiki) owns
research, experiment reports and observations. The [ownership policy](documentation-policy.md#canonical-ownership)
defines migration, provenance and evidence storage; each document has one editable home.

When documents disagree, the higher entry wins and the lower one is corrected:

| Priority | Source |
| --- | --- |
| 1 | [Product contract](PRODUCT-CONTRACT.md) |
| 2 | Accepted [architecture decisions](decisions/README.md) |
| 3 | Current [architecture views](architecture/README.md) |
| 4 | Registered protocols and [qualification](QUALIFICATION.md) in the repository; revision-specific results linked by the [evidence index](experiments/README.md) and pinned wiki commit |
| 5 | [Current state](CURRENT.md) |
| 6 | Research: [design consolidation](research/design-consolidation.md), [source synthesis](research/cross-system-source-synthesis.md), [ledger](research/ledger.md), [blueprint](architecture.md) and [research harnesses](../tools/research/README.md) |

Implementation is evidence of what exists; none of these documents overrides it silently. A disagreement between code and a document is reconciled explicitly ([documentation policy](documentation-policy.md#19-relationship-between-code-and-documentation)).

## Read in this order

1. [Current state](CURRENT.md): what is implemented, what is outstanding, and the known risks.
2. [Product contract](PRODUCT-CONTRACT.md) and [qualification](QUALIFICATION.md): the promises, the registered gates and the capability ledger.
3. [System](architecture/system.md), then the [architecture views](architecture/README.md).
4. [Verification strategy](formal/verification-strategy.md) and [verification matrix](formal/verification-matrix.md): which technique checks which claim.
5. [Concepts](concepts/README.md) and [glossary](glossary.md).
6. [Architecture decisions](decisions/README.md).
7. [Experiments](experiments/README.md): registered protocols and links to wiki results; [research wiki](https://github.com/kmosoti/FabricO11y/wiki): investigations and observations.
8. [Roadmap](ROADMAP.md) and the [milestone records](milestones/architecture-foundation.md).

[Operating FabricO11y](operations.md) covers installation, configuration, the scoped CLI and recovery states. The [learning path](LEARNING_PATH.md) follows the Rust ideas and system contracts step by step. The [blueprint](architecture.md) is a proposal and research agenda; its pipelines, guarantees and example numbers do not describe completed work.

Use the [contributor guide](CONTRIBUTING.md) for workflows and checks. [AGENTS.md](../AGENTS.md) is the operational contract for coding agents; the full [documentation policy](documentation-policy.md) applies to everyone.
